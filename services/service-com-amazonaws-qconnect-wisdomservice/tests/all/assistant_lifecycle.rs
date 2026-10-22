// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Amazon Q in Connect assistant + session slice.
//!
//! Every assertion goes over the wire: a request is built, dispatched through the same
//! [`MultiServiceRouter`] the production emulator mounts, and only the HTTP status, the
//! `x-amzn-errortype` header and the JSON body are inspected. Nothing reaches into the
//! service's state struct, so a response that is not computed from persisted state
//! cannot pass.
//!
//! The round-trip tests are TABLES: each row is one input along an axis the rule under
//! test generalises over (name form and length, description presence, tag count,
//! encryption configuration, tag-filter shape, AI-agent configuration, page size,
//! signing region). The `demo-assistant` case the bootstrap issue recorded is one row.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::state::{
    ASSISTANT_CAPABILITY_V2, ASSISTANT_STATUS_ACTIVE, DEFAULT_AI_AGENT_TYPES, MAX_ASSISTANTS,
    MAX_SESSIONS_PER_ASSISTANT, SESSION_ORIGIN_CUSTOMER,
};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every test signs with unless it is exercising the region rule itself.
const REGION: &str = "us-east-1";

/// `maxResults` is `range 1..100` on `ListAssistants`
/// (`smithy_query(query="operation",operation="ListAssistants")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// A UUID that matches the modelled `UuidWithQualifier` pattern, for an
/// `aiAgentConfiguration` entry. No AI agent operation is implemented yet, so the id is
/// only ever stored and reported.
const AI_AGENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A UUID no create ever mints, for the not-found assertions.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// The service's fixed sentence for each refusal condition these tests reach, written
/// out here rather than imported from `state.rs` so the assertion states the text
/// instead of restating the implementation's constant.
const MESSAGE_ASSISTANT_NOT_FOUND: &str = "Assistant does not exist.";
const MESSAGE_NAME_IN_USE: &str = "Name is already in use";
const MESSAGE_UNAUTHORIZED: &str = "Unauthorized";

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|v| !v.is_null())
    }

    fn string(&self, pointer: &str) -> Option<String> {
        self.field(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, region: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=deadbeef"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{region}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    async fn create_assistant(&self, region: &str, body: Value) -> Reply {
        self.send("POST", "/assistants", region, Some(body)).await
    }

    async fn get_assistant(&self, region: &str, label: &str) -> Reply {
        let label = encode_label(label);
        self.send("GET", &format!("/assistants/{label}"), region, None)
            .await
    }

    async fn list_assistants(&self, region: &str, query: &str) -> Reply {
        let uri = if query.is_empty() {
            "/assistants".to_string()
        } else {
            format!("/assistants?{query}")
        };
        self.send("GET", &uri, region, None).await
    }

    async fn delete_assistant(&self, region: &str, label: &str) -> Reply {
        let label = encode_label(label);
        self.send("DELETE", &format!("/assistants/{label}"), region, None)
            .await
    }

    async fn create_session(&self, region: &str, assistant: &str, body: Value) -> Reply {
        let assistant = encode_label(assistant);
        self.send(
            "POST",
            &format!("/assistants/{assistant}/sessions"),
            region,
            Some(body),
        )
        .await
    }

    async fn get_session(&self, region: &str, assistant: &str, session: &str) -> Reply {
        let assistant = encode_label(assistant);
        let session = encode_label(session);
        self.send(
            "GET",
            &format!("/assistants/{assistant}/sessions/{session}"),
            region,
            None,
        )
        .await
    }
}

/// A `UuidOrArn` label as it travels in a URL. A bare UUID is unchanged; an ARN's
/// slashes are percent-encoded, which is what the model's "URLs cannot contain the ARN"
/// note is about — an unencoded `/` would be a path separator and name another route.
fn encode_label(label: &str) -> String {
    label.replace('/', "%2F")
}

/// The assistant ARN AWS documents for this service:
/// `arn:aws:wisdom:<region>:<account>:assistant/<assistantId>`. Written out here rather
/// than taken from the handler's helper, so the test pins the format.
fn expected_assistant_arn(region: &str, assistant_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{region}:{account}:assistant/{assistant_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// The session ARN, which names the parent assistant as well as the session:
/// `arn:aws:wisdom:<region>:<account>:session/<assistantId>/<sessionId>`.
fn expected_session_arn(region: &str, assistant_id: &str, session_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{region}:{account}:session/{assistant_id}/{session_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// The modelled `Uuid` form: `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`.
fn is_modelled_uuid(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8usize, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(len, group)| {
                group.len() == *len
                    && group
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
}

/// One row of the assistant round-trip table: what a create is given, and therefore
/// what every later read must report.
#[derive(Debug, Clone)]
struct AssistantCase {
    /// What makes this row different from the others, for assertion messages.
    label: &'static str,
    name: String,
    description: Option<String>,
    tags: BTreeMap<String, String>,
    kms_key_id: Option<String>,
}

impl AssistantCase {
    fn create_body(&self) -> Value {
        let mut body = json!({ "name": self.name, "type": "AGENT" });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        if let Some(kms_key_id) = &self.kms_key_id {
            body["serverSideEncryptionConfiguration"] = json!({ "kmsKeyId": kms_key_id });
        }
        body
    }
}

fn assistant_case(
    label: &'static str,
    name: &str,
    description: Option<&str>,
    tags: &[(&str, &str)],
    kms_key_id: Option<&str>,
) -> AssistantCase {
    AssistantCase {
        label,
        name: name.to_string(),
        description: description.map(str::to_string),
        tags: tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        kms_key_id: kms_key_id.map(str::to_string),
    }
}

/// The table every assistant round-trip assertion runs over. Rows differ in name form
/// and length, whether a description is present and how long it is, tag count, and
/// whether an encryption configuration was given. `name` and `description` are
/// `len 1..255 pattern ^[a-zA-Z0-9\s_.,-]+`
/// (`smithy_query(query="operation",operation="CreateAssistant")`), so the rows include
/// both published bounds.
fn assistant_cases() -> Vec<AssistantCase> {
    vec![
        // The case the bootstrap issue recorded: one row of many.
        assistant_case(
            "issue's recorded case",
            "demo-assistant",
            Some("bootstrap demo"),
            &[("env", "test")],
            None,
        ),
        assistant_case(
            "shortest legal name, no description, no tags",
            "a",
            None,
            &[],
            None,
        ),
        assistant_case(
            "every character class the name pattern allows",
            "Mixed Case_name.9,-",
            Some("A description, with 9 classes_. -"),
            &[("team_1", "alpha"), ("a.b", "beta"), ("p/q", "gamma")],
            Some("arn:aws:kms:us-east-1:000000000000:key/22222222-3333-4444-5555-666666666666"),
        ),
        assistant_case(
            "longest legal name and description",
            &"n".repeat(255),
            Some(&"d".repeat(255)),
            &[("k", "v")],
            Some("alias/aws/wisdom"),
        ),
    ]
}

/// Assert a reply reports exactly the assistant `expected` was created with.
///
/// The status is pinned to the one terminal value: the service answers a create with the
/// assistant already `ACTIVE`, so there is no transition to race with and every read
/// reports the same status as the create.
fn assert_reported_assistant(
    label: &str,
    reply: &Reply,
    expected: &AssistantCase,
    assistant_id: &str,
    region: &str,
) {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("/assistant/assistantId").as_deref(),
        Some(assistant_id),
        "case {label:?}: expected the stored assistantId, got {:?}",
        reply.string("/assistant/assistantId")
    );
    assert_eq!(
        reply.string("/assistant/assistantArn"),
        Some(expected_assistant_arn(region, assistant_id)),
        "case {label:?}: expected the stored assistantArn, got {:?}",
        reply.string("/assistant/assistantArn")
    );
    assert_eq!(
        reply.string("/assistant/name").as_deref(),
        Some(expected.name.as_str()),
        "case {label:?}: expected the stored name, got {:?}",
        reply.string("/assistant/name")
    );
    assert_eq!(
        reply.string("/assistant/type").as_deref(),
        Some("AGENT"),
        "case {label:?}: expected type AGENT, got {:?}",
        reply.string("/assistant/type")
    );
    let status = reply.string("/assistant/status");
    assert_eq!(
        status.as_deref(),
        Some(ASSISTANT_STATUS_ACTIVE),
        "case {label:?}: expected {ASSISTANT_STATUS_ACTIVE}, got {status:?}"
    );
    assert_eq!(
        reply.string("/assistant/description"),
        expected.description.clone(),
        "case {label:?}: expected the stored description, got {:?}",
        reply.string("/assistant/description")
    );
    let reported_tags = reply.field("/assistant/tags").cloned();
    let expected_tags = Some(json!(expected.tags));
    assert_eq!(
        reported_tags, expected_tags,
        "case {label:?}: expected the stored tags {expected_tags:?} (the `tags` member is \
         reported for every assistant, empty when it carries none), got {reported_tags:?}"
    );
    let reported_key = reply.string("/assistant/serverSideEncryptionConfiguration/kmsKeyId");
    assert_eq!(
        reported_key, expected.kms_key_id,
        "case {label:?}: expected the stored kmsKeyId {:?}, got {reported_key:?}",
        expected.kms_key_id
    );
    assert_reported_assistant_configuration(label, reply);
}

/// The configuration members of the reported assistant: the capability it was
/// provisioned with, the topic it streams through, a binding per default AI agent type
/// and an (empty) orchestrator list. `tests/all/assistant_configuration.rs` is where
/// each of those values is pinned; here they are only shown to be reported by every
/// operation that reports an assistant.
fn assert_reported_assistant_configuration(label: &str, reply: &Reply) {
    let capability = reply.string("/assistant/capabilityConfiguration/type");
    assert_eq!(
        capability.as_deref(),
        Some(ASSISTANT_CAPABILITY_V2),
        "case {label:?}: expected capabilityConfiguration.type {ASSISTANT_CAPABILITY_V2}, \
         got {capability:?}"
    );
    let topic = reply.string("/assistant/integrationConfiguration/topicIntegrationArn");
    assert!(
        topic
            .as_deref()
            .is_some_and(|arn| arn.starts_with("arn:aws:sns:")),
        "case {label:?}: expected the assistant's SNS topic ARN, got {topic:?}"
    );
    let agents = reply
        .body
        .pointer("/assistant/aiAgentConfiguration")
        .and_then(Value::as_object)
        .map(|bound| bound.len());
    assert_eq!(
        agents,
        Some(DEFAULT_AI_AGENT_TYPES.len()),
        "case {label:?}: expected a binding for each of the {} default AI agent types, \
         got {agents:?}",
        DEFAULT_AI_AGENT_TYPES.len()
    );
    let orchestrators = reply
        .body
        .pointer("/assistant/orchestratorConfigurationList")
        .cloned();
    assert_eq!(
        orchestrators,
        Some(json!([])),
        "case {label:?}: expected orchestratorConfigurationList reported empty, \
         got {orchestrators:?}"
    );
}

/// A create is observable: the assistant every read reports is the one the create
/// stored, for every row of the table — by id and by ARN, and in the list.
#[tokio::test]
async fn an_assistant_round_trips_through_every_read() {
    let emulator = Emulator::new();

    for case in assistant_cases() {
        let label = case.label;
        let created = emulator.create_assistant(REGION, case.create_body()).await;
        let assistant_id = created
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {label:?}: no assistantId in {}", created.body));
        assert!(
            is_modelled_uuid(&assistant_id),
            "case {label:?}: expected a lowercase-hex UUID assistantId, got {assistant_id:?}"
        );
        assert_reported_assistant(label, &created, &case, &assistant_id, REGION);

        // Read back by the bare id, and by the full ARN the create returned: the
        // modelled `UuidOrArn` label admits both and they name the same record.
        let by_id = emulator.get_assistant(REGION, &assistant_id).await;
        assert_reported_assistant(label, &by_id, &case, &assistant_id, REGION);
        let arn = expected_assistant_arn(REGION, &assistant_id);
        let by_arn = emulator.get_assistant(REGION, &arn).await;
        assert_reported_assistant(label, &by_arn, &case, &assistant_id, REGION);

        // And the same values appear in the list, whose summaries are built from the
        // same stored record.
        let listed = emulator.list_assistants(REGION, "").await;
        let summary = listed
            .field("/assistantSummaries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no assistantSummaries in {}", listed.body))
            .iter()
            .find(|summary| summary.get("assistantId").and_then(Value::as_str) == Some(&assistant_id))
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: {assistant_id} is not in the list"));
        let as_reply = Reply {
            status: listed.status,
            error_type: String::new(),
            body: json!({ "assistant": summary }),
        };
        assert_reported_assistant(label, &as_reply, &case, &assistant_id, REGION);
    }
}

/// The ARN a create mints names the region the request was signed for, for every row.
#[tokio::test]
async fn an_assistant_arn_names_the_signed_region() {
    let emulator = Emulator::new();

    for (index, region) in ["us-east-1", "eu-west-3", "ap-southeast-2"]
        .into_iter()
        .enumerate()
    {
        let name = format!("regional-assistant-{index}");
        let created = emulator
            .create_assistant(region, json!({ "name": name, "type": "AGENT" }))
            .await;
        let assistant_id = created
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {region:?}: no assistantId in {}", created.body));
        assert_eq!(
            created.string("/assistant/assistantArn"),
            Some(expected_assistant_arn(region, &assistant_id)),
            "case {region:?}: expected an ARN in the signed region, got {:?}",
            created.string("/assistant/assistantArn")
        );
        // The stored ARN does not change when a different region reads the record.
        let read = emulator.get_assistant(REGION, &assistant_id).await;
        assert_eq!(
            read.string("/assistant/assistantArn"),
            Some(expected_assistant_arn(region, &assistant_id)),
            "case {region:?}: a read from another region rewrote the ARN: {:?}",
            read.string("/assistant/assistantArn")
        );
    }
}

/// The service answers a create with the assistant already in its terminal `ACTIVE`
/// status, so a resource it returns ready stays ready: the create, every read and the
/// list all report the same status, with no transition in between.
#[tokio::test]
async fn a_created_assistant_is_active_and_stays_active() {
    let emulator = Emulator::new();

    let created = emulator
        .create_assistant(REGION, json!({ "name": "settling", "type": "AGENT" }))
        .await;
    let assistant_id = created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", created.body));
    assert_eq!(
        created.string("/assistant/status").as_deref(),
        Some(ASSISTANT_STATUS_ACTIVE),
        "case {:?}: expected {ASSISTANT_STATUS_ACTIVE} in the create's own answer, got {:?}",
        "the create's answer",
        created.string("/assistant/status")
    );

    for case in ["first read", "second read"] {
        let read = emulator.get_assistant(REGION, &assistant_id).await;
        assert_eq!(
            read.string("/assistant/status").as_deref(),
            Some(ASSISTANT_STATUS_ACTIVE),
            "case {case:?}: expected {ASSISTANT_STATUS_ACTIVE}, got {:?}",
            read.string("/assistant/status")
        );
    }

    // The list reports the same status as the read, out of the same record.
    let listed = emulator.list_assistants(REGION, "").await;
    let status = listed
        .field("/assistantSummaries")
        .and_then(Value::as_array)
        .and_then(|summaries| summaries.first())
        .and_then(|summary| summary.get("status"))
        .and_then(Value::as_str)
        .map(str::to_string);
    assert_eq!(
        status.as_deref(),
        Some(ASSISTANT_STATUS_ACTIVE),
        "case {:?}: expected the list to report the stored status, got {status:?}",
        "the list"
    );
}

/// One row of the session round-trip table.
#[derive(Debug, Clone)]
struct SessionCase {
    label: &'static str,
    name: String,
    description: Option<String>,
    tags: BTreeMap<String, String>,
    tag_filter: Option<Value>,
    ai_agent_configuration: Option<Value>,
}

impl SessionCase {
    fn create_body(&self) -> Value {
        let mut body = json!({ "name": self.name });
        if let Some(description) = &self.description {
            body["description"] = json!(description);
        }
        if !self.tags.is_empty() {
            body["tags"] = json!(self.tags);
        }
        if let Some(tag_filter) = &self.tag_filter {
            body["tagFilter"] = tag_filter.clone();
        }
        if let Some(ai_agent_configuration) = &self.ai_agent_configuration {
            body["aiAgentConfiguration"] = ai_agent_configuration.clone();
        }
        body
    }
}

/// The table every session round-trip assertion runs over. Rows differ in name form and
/// length, description presence, tag count, which arm of the `TagFilter` union is used
/// (`tagCondition` / `andConditions` / `orConditions`, from
/// `smithy_query(query="shape",shape="TagFilter")`) and whether an AI-agent
/// configuration is given.
fn session_cases() -> Vec<SessionCase> {
    vec![
        // The case the bootstrap issue recorded.
        SessionCase {
            label: "issue's recorded case",
            name: "demo-session".to_string(),
            description: Some("chat".to_string()),
            tags: BTreeMap::new(),
            tag_filter: None,
            ai_agent_configuration: None,
        },
        SessionCase {
            label: "shortest legal name, a leaf tag condition",
            name: "s".to_string(),
            description: None,
            tags: [("env".to_string(), "test".to_string())]
                .into_iter()
                .collect(),
            tag_filter: Some(json!({ "tagCondition": { "key": "env", "value": "test" } })),
            ai_agent_configuration: None,
        },
        SessionCase {
            label: "andConditions filter and an AI agent configuration",
            name: "Mixed Case_session.9,-".to_string(),
            description: Some("A session, described_. -".to_string()),
            tags: [
                ("a.b".to_string(), "1".to_string()),
                ("p/q".to_string(), "2".to_string()),
            ]
            .into_iter()
            .collect(),
            tag_filter: Some(json!({
                "andConditions": [{ "key": "a.b", "value": "1" }, { "key": "p/q" }]
            })),
            ai_agent_configuration: Some(json!({
                "ANSWER_RECOMMENDATION": { "aiAgentId": AI_AGENT_ID, "enabled": true }
            })),
        },
        SessionCase {
            label: "longest legal name, orConditions filter",
            name: "s".repeat(255),
            description: Some("d".repeat(255)),
            tags: BTreeMap::new(),
            tag_filter: Some(json!({
                "orConditions": [{ "tagCondition": { "key": "team_1", "value": "alpha" } }]
            })),
            ai_agent_configuration: Some(json!({
                "MANUAL_SEARCH": { "aiAgentId": AI_AGENT_ID }
            })),
        },
    ]
}

fn assert_reported_session(
    label: &str,
    reply: &Reply,
    expected: &SessionCase,
    assistant_id: &str,
    session_id: &str,
    region: &str,
) {
    assert_eq!(
        reply.status, 200,
        "case {label:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.string("/session/sessionId").as_deref(),
        Some(session_id),
        "case {label:?}: expected the stored sessionId, got {:?}",
        reply.string("/session/sessionId")
    );
    assert_eq!(
        reply.string("/session/sessionArn"),
        Some(expected_session_arn(region, assistant_id, session_id)),
        "case {label:?}: expected the stored sessionArn, got {:?}",
        reply.string("/session/sessionArn")
    );
    assert_eq!(
        reply.string("/session/name").as_deref(),
        Some(expected.name.as_str()),
        "case {label:?}: expected the stored name, got {:?}",
        reply.string("/session/name")
    );
    assert_eq!(
        reply.string("/session/description"),
        expected.description.clone(),
        "case {label:?}: expected the stored description, got {:?}",
        reply.string("/session/description")
    );
    assert_eq!(
        reply.string("/session/origin").as_deref(),
        Some(SESSION_ORIGIN_CUSTOMER),
        "case {label:?}: a session created through CreateSession has origin \
         {SESSION_ORIGIN_CUSTOMER}, got {:?}",
        reply.string("/session/origin")
    );
    let reported_tags = reply.field("/session/tags").cloned();
    let expected_tags = Some(json!(expected.tags));
    assert_eq!(
        reported_tags, expected_tags,
        "case {label:?}: expected the stored tags {expected_tags:?} (the `tags` member is \
         reported for every session, empty when it carries none), got {reported_tags:?}"
    );
    let reported_filter = reply.field("/session/tagFilter").cloned();
    assert_eq!(
        reported_filter, expected.tag_filter,
        "case {label:?}: expected the stored tagFilter {:?}, got {reported_filter:?}",
        expected.tag_filter
    );
    let reported_agents = reply.field("/session/aiAgentConfiguration").cloned();
    assert_eq!(
        reported_agents, expected.ai_agent_configuration,
        "case {label:?}: expected the stored aiAgentConfiguration {:?}, got {reported_agents:?}",
        expected.ai_agent_configuration
    );
}

/// A session created inside an assistant round-trips through `GetSession`, for every
/// row — addressed by the bare ids and by the ARNs the creates returned.
#[tokio::test]
async fn a_session_round_trips_under_its_parent_assistant() {
    let emulator = Emulator::new();
    let created_assistant = emulator
        .create_assistant(REGION, json!({ "name": "session-parent", "type": "AGENT" }))
        .await;
    let assistant_id = created_assistant
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", created_assistant.body));

    for case in session_cases() {
        let label = case.label;
        let created = emulator
            .create_session(REGION, &assistant_id, case.create_body())
            .await;
        let session_id = created
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {label:?}: no sessionId in {}", created.body));
        assert!(
            is_modelled_uuid(&session_id),
            "case {label:?}: expected a lowercase-hex UUID sessionId, got {session_id:?}"
        );
        assert_reported_session(
            label,
            &created,
            &case,
            &assistant_id,
            &session_id,
            REGION,
        );

        let read = emulator
            .get_session(REGION, &assistant_id, &session_id)
            .await;
        assert_reported_session(label, &read, &case, &assistant_id, &session_id, REGION);

        let by_arn = emulator
            .get_session(
                REGION,
                &expected_assistant_arn(REGION, &assistant_id),
                &expected_session_arn(REGION, &assistant_id, &session_id),
            )
            .await;
        assert_reported_session(label, &by_arn, &case, &assistant_id, &session_id, REGION);
    }
}

/// A session is reachable only through its own parent: the same id under another
/// assistant is not found, and neither is an id nothing created.
#[tokio::test]
async fn a_session_does_not_resolve_under_another_assistant() {
    let emulator = Emulator::new();
    let mut assistants = Vec::new();
    for index in 0..2 {
        let created = emulator
            .create_assistant(
                REGION,
                json!({ "name": format!("tenant-{index}"), "type": "AGENT" }),
            )
            .await;
        assistants.push(
            created
                .string("/assistant/assistantId")
                .unwrap_or_else(|| panic!("no assistantId in {}", created.body)),
        );
    }
    let created = emulator
        .create_session(REGION, &assistants[0], json!({ "name": "owned-session" }))
        .await;
    let session_id = created
        .string("/session/sessionId")
        .unwrap_or_else(|| panic!("no sessionId in {}", created.body));

    for (label, assistant, session) in [
        ("the session's own parent", &assistants[0], &session_id),
        ("another assistant", &assistants[1], &session_id),
        (
            "an assistant that was never created",
            &ABSENT_UUID.to_string(),
            &session_id,
        ),
        (
            "a session id that was never created",
            &assistants[0],
            &ABSENT_UUID.to_string(),
        ),
    ] {
        let reply = emulator.get_session(REGION, assistant, session).await;
        let expected_status = if label == "the session's own parent" {
            200
        } else {
            404
        };
        assert_eq!(
            reply.status, expected_status,
            "case {label:?}: expected HTTP {expected_status}, got {} with body {}",
            reply.status, reply.body
        );
        if expected_status == 404 {
            assert!(
                reply.error_type.contains("ResourceNotFoundException"),
                "case {label:?}: expected ResourceNotFoundException, got {:?} with body {}",
                reply.error_type,
                reply.body
            );
        }
    }
}

/// Reading or deleting an id this account never created is `ResourceNotFoundException`
/// (HTTP 404), whichever legal label form names it — and an ARN this emulator could not
/// have minted names no resource even when its trailing UUID is a live assistant's.
#[tokio::test]
async fn an_unknown_assistant_is_not_found() {
    let emulator = Emulator::new();
    let created = emulator
        .create_assistant(REGION, json!({ "name": "addressed", "type": "AGENT" }))
        .await;
    let live_id = created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", created.body));

    for (label, addressed) in [
        ("bare uuid", ABSENT_UUID.to_string()),
        (
            "full arn",
            expected_assistant_arn(REGION, ABSENT_UUID),
        ),
        // A live assistant's own UUID, carried by an ARN that names another account:
        // the ARN does not name this resource, so it resolves to nothing.
        (
            "another account's arn for a live id",
            format!("arn:aws:wisdom:{REGION}:123456789012:assistant/{live_id}"),
        ),
        // Same for an ARN whose region segment names a region nothing was created in
        // but whose account is foreign — the whole prefix must be one this emulator
        // mints.
        (
            "another account's arn in another region",
            format!("arn:aws:wisdom:eu-west-3:123456789012:assistant/{live_id}"),
        ),
    ] {
        for (op, reply) in [
            ("GetAssistant", emulator.get_assistant(REGION, &addressed).await),
            (
                "DeleteAssistant",
                emulator.delete_assistant(REGION, &addressed).await,
            ),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {label:?}/{op}: expected HTTP 404, got {} with body {}",
                reply.status, reply.body
            );
            assert!(
                reply.error_type.contains("ResourceNotFoundException"),
                "case {label:?}/{op}: expected ResourceNotFoundException, got {:?} with body {}",
                reply.error_type,
                reply.body
            );
            // The service's fixed text for the condition, naming nothing from the
            // request; the label the request used travels in `resourceName`.
            assert_eq!(
                reply.string("/message").as_deref(),
                Some(MESSAGE_ASSISTANT_NOT_FOUND),
                "case {label:?}/{op}: expected {MESSAGE_ASSISTANT_NOT_FOUND:?}, got {:?}",
                reply.string("/message")
            );
            assert_eq!(
                reply.string("/resourceName").as_deref(),
                Some(addressed.as_str()),
                "case {label:?}/{op}: expected resourceName to name the addressed label, \
                 got {:?}",
                reply.string("/resourceName")
            );
        }
    }

    // And the assistant those foreign ARNs named is untouched: nothing deleted it.
    let still_there = emulator.get_assistant(REGION, &live_id).await;
    assert_eq!(
        still_there.status, 200,
        "case {:?}: expected the live assistant to survive, got {} with body {}",
        "live assistant after foreign-ARN addressing", still_there.status, still_there.body
    );
}

/// Deleting an assistant forgets it and every session it owned, frees its name, and is
/// reported once: a second delete of the same id is not found.
#[tokio::test]
async fn deleting_an_assistant_forgets_it_and_its_sessions() {
    let emulator = Emulator::new();

    // Two assistants, so the delete is shown to touch only its own.
    let mut ids = Vec::new();
    for index in 0..2 {
        let created = emulator
            .create_assistant(
                REGION,
                json!({ "name": format!("deletable-{index}"), "type": "AGENT" }),
            )
            .await;
        ids.push(
            created
                .string("/assistant/assistantId")
                .unwrap_or_else(|| panic!("no assistantId in {}", created.body)),
        );
    }
    let mut sessions = Vec::new();
    for index in 0..3 {
        let created = emulator
            .create_session(
                REGION,
                &ids[0],
                json!({ "name": format!("doomed-session-{index}") }),
            )
            .await;
        sessions.push(
            created
                .string("/session/sessionId")
                .unwrap_or_else(|| panic!("no sessionId in {}", created.body)),
        );
    }

    let deleted = emulator.delete_assistant(REGION, &ids[0]).await;
    assert_eq!(
        deleted.status, 204,
        "case {:?}: expected HTTP 204, got {} with body {}",
        "delete", deleted.status, deleted.body
    );
    // A 204 carries no members: the framework renders this service's empty
    // `DeleteAssistantResponse` as an empty JSON object.
    let members = match &deleted.body {
        Value::Null => 0,
        Value::Object(members) => members.len(),
        other => panic!("case {:?}: expected an empty body, got {other}", "delete"),
    };
    assert_eq!(
        members, 0,
        "case {:?}: expected a body with no members, got {}",
        "delete", deleted.body
    );

    for (label, reply) in [
        ("GetAssistant", emulator.get_assistant(REGION, &ids[0]).await),
        (
            "DeleteAssistant again",
            emulator.delete_assistant(REGION, &ids[0]).await,
        ),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {label:?}: expected HTTP 404 after the delete, got {} with body {}",
            reply.status, reply.body
        );
    }
    for session_id in &sessions {
        let reply = emulator.get_session(REGION, &ids[0], session_id).await;
        assert_eq!(
            reply.status, 404,
            "case {session_id:?}: expected HTTP 404 for a session of a deleted assistant, \
             got {} with body {}",
            reply.status, reply.body
        );
    }

    let listed = emulator.list_assistants(REGION, "").await;
    let reported: Vec<String> = listed
        .field("/assistantSummaries")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|summary| summary.get("assistantId").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert_eq!(
        reported,
        vec![ids[1].clone()],
        "case {:?}: expected only the surviving assistant in the list, got {reported:?}",
        "list after delete"
    );

    // The deleted assistant's name is free again — the index released it with it.
    let recreated = emulator
        .create_assistant(REGION, json!({ "name": "deletable-0", "type": "AGENT" }))
        .await;
    assert_eq!(
        recreated.status, 200,
        "case {:?}: expected the freed name to be reusable, got {} with body {}",
        "recreate under the deleted name", recreated.status, recreated.body
    );
}

/// A second live assistant cannot take a name an existing one holds: that is the
/// conflict `ConflictException` (HTTP 409) describes.
#[tokio::test]
async fn a_duplicate_assistant_name_conflicts() {
    let emulator = Emulator::new();

    for (label, name) in [
        ("plain name", "unique-assistant".to_string()),
        ("longest legal name", "n".repeat(255)),
    ] {
        let first = emulator
            .create_assistant(REGION, json!({ "name": name, "type": "AGENT" }))
            .await;
        assert_eq!(
            first.status, 200,
            "case {label:?}: expected the first create to succeed, got {} with body {}",
            first.status, first.body
        );
        let again = emulator
            .create_assistant(REGION, json!({ "name": name, "type": "AGENT" }))
            .await;
        assert_eq!(
            again.status, 409,
            "case {label:?}: expected HTTP 409 for a duplicate name, got {} with body {}",
            again.status, again.body
        );
        assert_eq!(
            again.string("/message").as_deref(),
            Some(MESSAGE_NAME_IN_USE),
            "case {label:?}: expected the service's fixed text {MESSAGE_NAME_IN_USE:?}, got {:?}",
            again.string("/message")
        );
        assert!(
            again.error_type.contains("ConflictException"),
            "case {label:?}: expected ConflictException, got {:?} with body {}",
            again.error_type,
            again.body
        );
    }
}

/// Both creates are `@idempotent` with an `@idempotencyToken`: replaying a token answers
/// with the resource it already created instead of making a second one.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_resource() {
    let emulator = Emulator::new();

    for (label, token) in [
        ("short token", "t".to_string()),
        ("uuid-shaped token", ABSENT_UUID.to_string()),
        ("longest legal token", "x".repeat(4096)),
    ] {
        let name = format!("idempotent-{}", token.len());
        let first = emulator
            .create_assistant(
                REGION,
                json!({ "name": name, "type": "AGENT", "clientToken": token }),
            )
            .await;
        let assistant_id = first
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {label:?}: no assistantId in {}", first.body));
        let replay = emulator
            .create_assistant(
                REGION,
                json!({ "name": name, "type": "AGENT", "clientToken": token }),
            )
            .await;
        assert_eq!(
            replay.status, 200,
            "case {label:?}: expected the replay to succeed, got {} with body {}",
            replay.status, replay.body
        );
        assert_eq!(
            replay.string("/assistant/assistantId").as_deref(),
            Some(assistant_id.as_str()),
            "case {label:?}: expected the replay to answer with {assistant_id}, got {:?}",
            replay.string("/assistant/assistantId")
        );

        let first_session = emulator
            .create_session(
                REGION,
                &assistant_id,
                json!({ "name": "idempotent-session", "clientToken": token }),
            )
            .await;
        let session_id = first_session
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {label:?}: no sessionId in {}", first_session.body));
        let replayed_session = emulator
            .create_session(
                REGION,
                &assistant_id,
                json!({ "name": "idempotent-session", "clientToken": token }),
            )
            .await;
        assert_eq!(
            replayed_session.string("/session/sessionId").as_deref(),
            Some(session_id.as_str()),
            "case {label:?}: expected the replay to answer with {session_id}, got {:?}",
            replayed_session.string("/session/sessionId")
        );

        // And exactly one session exists under the assistant for that token.
        let read = emulator
            .get_session(REGION, &assistant_id, &session_id)
            .await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: expected the single created session to be readable, got {}",
            read.status
        );

        // The token is scoped per create: having named a session, it still names the
        // assistant it created on `CreateAssistant`.
        let assistant_again = emulator
            .create_assistant(
                REGION,
                json!({ "name": name, "type": "AGENT", "clientToken": token }),
            )
            .await;
        assert_eq!(
            assistant_again.string("/assistant/assistantId").as_deref(),
            Some(assistant_id.as_str()),
            "case {label:?}: expected the token to still name {assistant_id} on \
             CreateAssistant, got {:?}",
            assistant_again.string("/assistant/assistantId")
        );
    }
}

/// Creating a session under an assistant this account does not hold is refused before
/// anything is minted — with the modelled `UnauthorizedException` (HTTP 401), which is
/// what this path answers for a parent it cannot resolve, whichever legal label form
/// names that parent, including an ARN this emulator could not have minted (another
/// account, another partition), which names no resource of this account.
#[tokio::test]
async fn creating_a_session_under_an_unknown_assistant_is_refused() {
    let emulator = Emulator::new();
    // A live assistant, so a foreign-account ARN carrying a REAL assistant id is covered
    // too: the ARN names no assistant of this account, so the create is refused the same
    // way as one naming an id nothing holds.
    let live = emulator
        .create_assistant(REGION, json!({ "name": "live-parent", "type": "AGENT" }))
        .await;
    let live_id = live
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", live.body));

    for (label, parent) in [
        ("bare uuid nothing holds", ABSENT_UUID.to_string()),
        (
            "this account's arn for an absent id",
            expected_assistant_arn(REGION, ABSENT_UUID),
        ),
        (
            "another account's arn for a live id",
            format!("arn:aws:wisdom:{REGION}:123456789012:assistant/{live_id}"),
        ),
        (
            "another partition's arn for a live id",
            format!(
                "arn:aws-cn:wisdom:{REGION}:{}:assistant/{live_id}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
    ] {
        let reply = emulator
            .create_session(REGION, &parent, json!({ "name": "orphan" }))
            .await;
        assert_eq!(
            reply.status, 401,
            "case {label:?}: expected HTTP 401, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_type.contains("UnauthorizedException"),
            "case {label:?}: expected UnauthorizedException, got {:?} with body {}",
            reply.error_type,
            reply.body
        );
        assert_eq!(
            reply.string("/message").as_deref(),
            Some(MESSAGE_UNAUTHORIZED),
            "case {label:?}: expected the service's fixed text {MESSAGE_UNAUTHORIZED:?}, \
             got {:?}",
            reply.string("/message")
        );
    }

    // And the assistant those foreign ARNs named survives the refusals.
    let sessions_parent = emulator.get_assistant(REGION, &live_id).await;
    assert_eq!(
        sessions_parent.status, 200,
        "case \"live parent after the refusals\": expected HTTP 200, got {} with body {}",
        sessions_parent.status, sessions_parent.body
    );
}

/// `ListAssistants` pages over every stored assistant exactly once at any page size,
/// and hands back no token on the last page. The page sizes cover both published bounds
/// of `maxResults` (1..100) and sizes between.
#[tokio::test]
async fn the_list_pages_over_every_assistant_exactly_once() {
    let emulator = Emulator::new();

    let mut created = Vec::new();
    for index in 0..7 {
        let reply = emulator
            .create_assistant(
                REGION,
                json!({ "name": format!("paged-{index}"), "type": "AGENT" }),
            )
            .await;
        created.push(
            reply
                .string("/assistant/assistantId")
                .unwrap_or_else(|| panic!("no assistantId in {}", reply.body)),
        );
    }
    created.sort();

    for page_size in [MAX_RESULTS_MIN, 2, 3, 7, 8, MAX_RESULTS_MAX] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut calls = 0;
        loop {
            calls += 1;
            assert!(
                calls <= 16,
                "case {page_size:?}: the token chain did not terminate after {calls} calls"
            );
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let reply = emulator.list_assistants(REGION, &query).await;
            assert_eq!(
                reply.status, 200,
                "case {page_size:?}: expected HTTP 200, got {} with body {}",
                reply.status, reply.body
            );
            let page: Vec<String> = reply
                .field("/assistantSummaries")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {page_size:?}: no assistantSummaries in {}", reply.body)
                })
                .iter()
                .filter_map(|summary| summary.get("assistantId").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            let limit = usize::try_from(page_size).expect("a positive page size");
            assert!(
                page.len() <= limit,
                "case {page_size:?}: expected at most {limit} summaries, got {}",
                page.len()
            );
            seen.extend(page);
            token = reply.string("/nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, created,
            "case {page_size:?}: expected every assistant exactly once in id order, got {seen:?}"
        );
    }
}

/// A `maxResults` outside the published range is refused, and a `nextToken` this
/// service never minted is refused rather than silently read as "from the beginning".
#[tokio::test]
async fn the_list_refuses_a_page_request_it_cannot_serve() {
    let emulator = Emulator::new();
    emulator
        .create_assistant(REGION, json!({ "name": "listed", "type": "AGENT" }))
        .await;

    // One past each published bound of `maxResults` (1..100).
    for (label, query) in [
        ("below the minimum", format!("maxResults={}", MAX_RESULTS_MIN - 1)),
        ("above the maximum", format!("maxResults={}", MAX_RESULTS_MAX + 1)),
        ("a token nothing minted", "nextToken=not-a-token".to_string()),
        // Hex-decodable but not one of this service's cursors: still refused.
        ("a token with no cursor scope", "nextToken=6161".to_string()),
    ] {
        let reply = emulator.list_assistants(REGION, &query).await;
        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            reply.status, reply.body
        );
    }

    // And each published bound itself is accepted.
    for bound in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let reply = emulator
            .list_assistants(REGION, &format!("maxResults={bound}"))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {bound:?}: expected a published bound to be accepted, got {} with body {}",
            reply.status, reply.body
        );
    }
}

/// An empty account lists no assistants and hands back no token — the required
/// `assistantSummaries` member is still present.
#[tokio::test]
async fn an_empty_account_lists_an_empty_page() {
    let emulator = Emulator::new();
    let reply = emulator.list_assistants(REGION, "").await;

    assert_eq!(
        reply.status, 200,
        "case {:?}: expected HTTP 200, got {} with body {}",
        "empty list", reply.status, reply.body
    );
    let summaries = reply
        .field("/assistantSummaries")
        .and_then(Value::as_array)
        .map(Vec::len);
    assert_eq!(
        summaries,
        Some(0),
        "case {:?}: expected an empty assistantSummaries list, got {:?}",
        "empty list",
        reply.body
    );
    assert_eq!(
        reply.string("/nextToken"),
        None,
        "case {:?}: expected no nextToken on a last page, got {:?}",
        "empty list",
        reply.string("/nextToken")
    );
}

/// The per-account assistant bound is the store's own ceiling: every create up to and
/// including it succeeds, and the one past it is refused with the modelled
/// `ServiceQuotaExceededException` (402) — after which deleting one frees a slot.
#[tokio::test]
async fn an_account_at_its_assistant_bound_refuses_one_more() {
    let emulator = Emulator::new();

    let mut last_id = String::new();
    for index in 0..MAX_ASSISTANTS {
        let reply = emulator
            .create_assistant(
                REGION,
                json!({ "name": format!("bounded-{index}"), "type": "AGENT" }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {index:?}: expected every create up to the bound to succeed, got {} \
             with body {}",
            reply.status, reply.body
        );
        last_id = reply
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {index:?}: no assistantId in {}", reply.body));
    }

    let refused = emulator
        .create_assistant(REGION, json!({ "name": "one-too-many", "type": "AGENT" }))
        .await;
    assert_eq!(
        refused.status, 402,
        "case {:?}: expected HTTP 402 past the bound, got {} with body {}",
        "one past the bound", refused.status, refused.body
    );
    assert!(
        refused
            .error_type
            .contains("ServiceQuotaExceededException"),
        "case {:?}: expected ServiceQuotaExceededException, got {:?} with body {}",
        "one past the bound",
        refused.error_type,
        refused.body
    );

    // A delete frees a slot: the bound is on what is held, not on what was ever made.
    emulator.delete_assistant(REGION, &last_id).await;
    let accepted = emulator
        .create_assistant(REGION, json!({ "name": "one-too-many", "type": "AGENT" }))
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {:?}: expected a freed slot to be reusable, got {} with body {}",
        "after a delete", accepted.status, accepted.body
    );
}

/// The per-assistant session bound behaves the same way; `CreateSession` models no
/// quota error, so the refusal is its modelled `ValidationException` (400).
#[tokio::test]
async fn an_assistant_at_its_session_bound_refuses_one_more() {
    let emulator = Emulator::new();
    let created = emulator
        .create_assistant(REGION, json!({ "name": "session-bounded", "type": "AGENT" }))
        .await;
    let assistant_id = created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("no assistantId in {}", created.body));

    for index in 0..MAX_SESSIONS_PER_ASSISTANT {
        let reply = emulator
            .create_session(
                REGION,
                &assistant_id,
                json!({ "name": format!("bounded-session-{index}") }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {index:?}: expected every create up to the bound to succeed, got {} \
             with body {}",
            reply.status, reply.body
        );
    }

    let refused = emulator
        .create_session(REGION, &assistant_id, json!({ "name": "one-too-many" }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case {:?}: expected HTTP 400 past the bound, got {} with body {}",
        "one past the bound", refused.status, refused.body
    );
    assert!(
        refused.error_type.contains("ValidationException"),
        "case {:?}: expected ValidationException, got {:?} with body {}",
        "one past the bound",
        refused.error_type,
        refused.body
    );
}
