// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REVISION the service's own AI agent bindings name.
//!
//! The rule under test: a binding the SERVICE made for an `AIAgentType` names the
//! service's built-in agent for that type by that agent's id plus the qualifier of the
//! revision the service bound — the floating `$LATEST` alias where the service binds the
//! agent itself, a published version number where it pins one — and that qualified id is
//! a service constant: every path that projects an `aiAgentConfiguration` map reports the
//! same pair, for every assistant and every session, whatever the request asked for and
//! whatever region it was signed for. An id a CALLER configured is reported exactly as
//! given, qualifier and all.
//!
//! The tests are TABLES. The assistant rows differ along the create input (name form and
//! length, description, tag count, encryption configuration, signing region) and the
//! session rows along what the create configures for the session itself (nothing, another
//! type, the floating type, several types at once) — none of which is what decides the
//! revision the service bound. Every `AIAgentType` with a built-in agent is asserted, not
//! only the one the divergence was reported on.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts; nothing reaches into the service's state struct.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a row signs with unless it is exercising the region axis itself.
const REGION: &str = "us-east-1";

/// The qualifier that names an AI agent's floating alias — the agent itself rather than
/// one of its published versions. The modelled label pattern admits it
/// (`smithy_query(query="shape",shape="UuidWithQualifier")`:
/// `^[a-f0-9]{8}-…-[a-f0-9]{12}(:[A-Z0-9_$]+){0,1}$`).
const QUALIFIER_LATEST: &str = "$LATEST";

/// The service's built-in AI agent for each `AIAgentType`: the agent's own uuid and the
/// qualifier the service's binding names it by.
///
/// SOURCE: the recorded wire answers of `CreateAssistant`, `CreateSession` and
/// `GetSession`, which carry the same pairs across independently created assistants. One
/// entry per `AIAgentType` the model declares
/// (`smithy_query(query="shape",shape="AIAgentType")`) except `ORCHESTRATION`, which the
/// service publishes no built-in agent for.
const BUILT_IN_AGENTS: [(&str, &str, &str); 8] = [
    (
        "ANSWER_RECOMMENDATION",
        "3025a3fa-5419-4c9c-a040-877213b93f70",
        QUALIFIER_LATEST,
    ),
    (
        "CASE_SUMMARIZATION",
        "9114812c-2a1e-4501-8817-daec4223ce57",
        "3",
    ),
    (
        "EMAIL_GENERATIVE_ANSWER",
        "4d7e9f2a-1b3c-5d6e-8f7a-9b0c1d2e3f4a",
        "7",
    ),
    (
        "EMAIL_OVERVIEW",
        "91bb967b-bd5e-4e02-a8d5-cb7c5b2bf9c7",
        "5",
    ),
    (
        "EMAIL_RESPONSE",
        "8f4e7d2a-9c31-4b5e-a6f8-e2d9c7b54321",
        "7",
    ),
    (
        "MANUAL_SEARCH",
        "652f0287-1610-4fee-bbe3-c687c13f5ec3",
        "9",
    ),
    (
        "NOTE_TAKING",
        "7fc84960-5b46-4e44-9dd6-5956b2acfaa4",
        "3",
    ),
    (
        "SELF_SERVICE",
        "529207fe-80fd-42be-bcf2-facd2141aec7",
        "5",
    ),
];

/// The `sessionId` of the session the service manages for every assistant, which reports
/// the assistant's own bindings.
const SYSTEM_SESSION_ID: &str = "00000000-0000-0000-0000-000000000000";

/// The `Origin` of a resource Amazon Q in Connect created itself
/// (`smithy_query(query="shape",shape="Origin")`), which is what a built-in agent reports.
const ORIGIN_SYSTEM: &str = "SYSTEM";

/// Ids a caller may configure, spanning the forms the modelled `UuidWithQualifier`
/// pattern admits: a bare uuid, one pinned to a numbered version, and one carrying the
/// floating alias.
const CALLER_AGENT_BARE: &str = "11111111-2222-3333-4444-555555555555";
const CALLER_AGENT_VERSIONED: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee:7";
const CALLER_AGENT_LATEST: &str = "0123abcd-4567-89ef-0123-456789abcdef:$LATEST";

/// One wire answer, reduced to what a client reads.
#[derive(Debug)]
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn field(&self, pointer: &str) -> Option<&Value> {
        self.body.pointer(pointer).filter(|value| !value.is_null())
    }

    fn string(&self, pointer: &str) -> Option<String> {
        self.field(pointer)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn object(&self, pointer: &str) -> Option<Map<String, Value>> {
        self.field(pointer).and_then(Value::as_object).cloned()
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
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    async fn create_assistant(&self, region: &str, body: Value) -> Reply {
        self.send("POST", "/assistants", region, Some(body)).await
    }

    async fn get_assistant(&self, region: &str, assistant_id: &str) -> Reply {
        self.send("GET", &format!("/assistants/{assistant_id}"), region, None)
            .await
    }

    async fn list_assistants(&self, region: &str) -> Reply {
        self.send("GET", "/assistants", region, None).await
    }

    async fn create_session(&self, region: &str, assistant_id: &str, body: Value) -> Reply {
        self.send(
            "POST",
            &format!("/assistants/{assistant_id}/sessions"),
            region,
            Some(body),
        )
        .await
    }

    async fn get_session(&self, region: &str, assistant_id: &str, session_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}/sessions/{session_id}"),
            region,
            None,
        )
        .await
    }

    async fn get_ai_agent(&self, region: &str, assistant_id: &str, ai_agent_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/assistants/{assistant_id}/aiagents/{ai_agent_id}"),
            region,
            None,
        )
        .await
    }
}

/// The `aiAgentId` the service's binding for `agent_type` must report: the built-in
/// agent's id qualified by the revision the service bound.
fn expected_binding_id(agent_type: &str) -> String {
    let (_, ai_agent_id, qualifier) = BUILT_IN_AGENTS
        .iter()
        .find(|(candidate, _, _)| *candidate == agent_type)
        .unwrap_or_else(|| panic!("{agent_type} is no built-in AI agent type"));
    format!("{ai_agent_id}:{qualifier}")
}

/// Assert every service-made binding of one projected `aiAgentConfiguration` map, for
/// every type the caller did not configure itself.
fn assert_service_bindings(case: &str, path: &str, bindings: &Map<String, Value>) {
    for (agent_type, _, qualifier) in BUILT_IN_AGENTS {
        let reported = bindings
            .get(agent_type)
            .and_then(|binding| binding.get("aiAgentId"))
            .and_then(Value::as_str);
        let expected = expected_binding_id(agent_type);
        assert_eq!(
            reported,
            Some(expected.as_str()),
            "case {case:?}/{path}/{agent_type}: expected the service's binding \
             {expected:?}, got {reported:?}"
        );
        // The floating alias is not a version number, and a pinned revision is: the
        // divergence this suite pins is a binding that materialised a number where the
        // service names its agent's alias.
        let reported_qualifier = reported.and_then(|id| id.split_once(':')).map(|(_, q)| q);
        assert_eq!(
            reported_qualifier,
            Some(qualifier),
            "case {case:?}/{path}/{agent_type}: expected the qualifier {qualifier:?}, \
             got {reported_qualifier:?}"
        );
        let numeric = reported_qualifier.is_some_and(|q| q.parse::<i64>().is_ok());
        assert_eq!(
            numeric,
            qualifier != QUALIFIER_LATEST,
            "case {case:?}/{path}/{agent_type}: expected a numeric qualifier only where \
             the service pins a published version, got {reported_qualifier:?}"
        );
    }
}

/// One row of the assistant table: the create input differs, what the service binds does
/// not.
#[derive(Debug, Clone)]
struct AssistantRow {
    label: &'static str,
    region: &'static str,
    body: Value,
}

fn assistant_rows() -> Vec<AssistantRow> {
    vec![
        AssistantRow {
            label: "minimal create",
            region: REGION,
            body: json!({ "name": "revisions-minimal", "type": "AGENT" }),
        },
        AssistantRow {
            label: "described, three tags",
            region: REGION,
            body: json!({
                "name": "revisions.described_1",
                "type": "AGENT",
                "description": "Bindings, described. -",
                "tags": { "owner": "qconnect-test", "a.b": "beta", "cost-centre": "42" },
            }),
        },
        AssistantRow {
            label: "shortest legal name, another region",
            region: "eu-west-3",
            body: json!({ "name": "r", "type": "AGENT" }),
        },
        AssistantRow {
            label: "longest legal name, customer managed key, third region",
            region: "ap-southeast-2",
            body: json!({
                "name": "n".repeat(255),
                "type": "AGENT",
                "serverSideEncryptionConfiguration": { "kmsKeyId": "alias/aws/wisdom" },
            }),
        },
    ]
}

/// One row of the session table: what the create configures for the session itself, which
/// never re-qualifies a binding the service made for another type.
#[derive(Debug, Clone)]
struct SessionRow {
    label: &'static str,
    body: Value,
    /// The bindings this create configures, by `AIAgentType`: reported exactly as sent.
    configured: &'static [(&'static str, &'static str)],
}

fn session_rows() -> Vec<SessionRow> {
    vec![
        SessionRow {
            label: "configures nothing",
            body: json!({ "name": "inherits-everything" }),
            configured: &[],
        },
        SessionRow {
            label: "configures a bare uuid for another type",
            body: json!({
                "name": "overrides-manual-search",
                "aiAgentConfiguration": { "MANUAL_SEARCH": { "aiAgentId": CALLER_AGENT_BARE } },
            }),
            configured: &[("MANUAL_SEARCH", CALLER_AGENT_BARE)],
        },
        SessionRow {
            label: "pins a version for the floating type",
            body: json!({
                "name": "overrides-answer-recommendation",
                "aiAgentConfiguration": {
                    "ANSWER_RECOMMENDATION": { "aiAgentId": CALLER_AGENT_VERSIONED },
                },
            }),
            configured: &[("ANSWER_RECOMMENDATION", CALLER_AGENT_VERSIONED)],
        },
        SessionRow {
            label: "configures three types, one of them with the floating alias",
            body: json!({
                "name": "overrides-three",
                "aiAgentConfiguration": {
                    "SELF_SERVICE": { "aiAgentId": CALLER_AGENT_LATEST },
                    "NOTE_TAKING": { "aiAgentId": CALLER_AGENT_BARE },
                    "EMAIL_RESPONSE": { "aiAgentId": CALLER_AGENT_VERSIONED },
                },
            }),
            configured: &[
                ("SELF_SERVICE", CALLER_AGENT_LATEST),
                ("NOTE_TAKING", CALLER_AGENT_BARE),
                ("EMAIL_RESPONSE", CALLER_AGENT_VERSIONED),
            ],
        },
    ]
}

/// Create one assistant and hand back its id, failing the case if the create did not.
async fn created_assistant_id(emulator: &Emulator, row: &AssistantRow) -> String {
    let created = emulator
        .create_assistant(row.region, row.body.clone())
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from the create, got {} with body {}",
        row.label, created.status, created.body
    );
    created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("case {:?}: no assistantId in {}", row.label, created.body))
}

/// The bindings one reply projects at `pointer`, failing the case when it projects none.
fn bindings_at(case: &str, path: &str, reply: &Reply, pointer: &str) -> Map<String, Value> {
    reply.object(pointer).unwrap_or_else(|| {
        panic!(
            "case {case:?}/{path}: expected an aiAgentConfiguration at {pointer}, got {}",
            reply.body
        )
    })
}

/// Every write and read path that projects an assistant reports the service's bindings
/// with the revision the service bound, for every row.
#[tokio::test]
async fn every_assistant_path_reports_the_service_bound_revision() {
    for row in assistant_rows() {
        // One emulator per row, so the list below reports exactly this row's assistant.
        let emulator = Emulator::new();
        let case = row.label;
        let created = emulator
            .create_assistant(row.region, row.body.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {} with body {}",
            created.status, created.body
        );
        let assistant_id = created
            .string("/assistant/assistantId")
            .unwrap_or_else(|| panic!("case {case:?}: no assistantId in {}", created.body));
        assert_service_bindings(
            case,
            "CreateAssistant",
            &bindings_at(
                case,
                "CreateAssistant",
                &created,
                "/assistant/aiAgentConfiguration",
            ),
        );

        let read = emulator.get_assistant(row.region, &assistant_id).await;
        assert_service_bindings(
            case,
            "GetAssistant",
            &bindings_at(case, "GetAssistant", &read, "/assistant/aiAgentConfiguration"),
        );

        let listed = emulator.list_assistants(row.region).await;
        assert_service_bindings(
            case,
            "ListAssistants",
            &bindings_at(
                case,
                "ListAssistants",
                &listed,
                "/assistantSummaries/0/aiAgentConfiguration",
            ),
        );

        // The session the service manages for the assistant projects the same map.
        let system = emulator
            .get_session(row.region, &assistant_id, SYSTEM_SESSION_ID)
            .await;
        assert_service_bindings(
            case,
            "GetSession(system)",
            &bindings_at(
                case,
                "GetSession(system)",
                &system,
                "/session/aiAgentConfiguration",
            ),
        );
    }
}

/// A session reports the service's bindings with the same revision for every type its own
/// create did not configure, on the create's answer and on every later read; a type the
/// create DID configure is reported exactly as the caller sent it.
#[tokio::test]
async fn a_session_keeps_the_service_bound_revision_it_did_not_configure() {
    let emulator = Emulator::new();
    let assistant_row = AssistantRow {
        label: "session owner",
        region: REGION,
        body: json!({ "name": "session-revisions", "type": "AGENT" }),
    };
    let assistant_id = created_assistant_id(&emulator, &assistant_row).await;

    for row in session_rows() {
        let case = row.label;
        let created = emulator
            .create_session(REGION, &assistant_id, row.body.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the session create, got {} with body {}",
            created.status, created.body
        );
        let session_id = created
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {}", created.body));
        let read = emulator
            .get_session(REGION, &assistant_id, &session_id)
            .await;

        for (path, reply) in [("CreateSession", &created), ("GetSession", &read)] {
            let bindings = bindings_at(case, path, reply, "/session/aiAgentConfiguration");
            // Every type the create left alone still carries the service's binding.
            let mut inherited = bindings.clone();
            for (agent_type, configured_id) in row.configured {
                let reported = inherited
                    .remove(*agent_type)
                    .and_then(|binding| binding.get("aiAgentId").cloned())
                    .and_then(|id| id.as_str().map(str::to_string));
                assert_eq!(
                    reported.as_deref(),
                    Some(*configured_id),
                    "case {case:?}/{path}/{agent_type}: expected the id the caller \
                     configured ({configured_id:?}), got {reported:?}"
                );
            }
            for (agent_type, _, _) in BUILT_IN_AGENTS {
                if row
                    .configured
                    .iter()
                    .any(|(configured, _)| *configured == agent_type)
                {
                    continue;
                }
                let expected = expected_binding_id(agent_type);
                let reported = inherited
                    .get(agent_type)
                    .and_then(|binding| binding.get("aiAgentId"))
                    .and_then(Value::as_str);
                assert_eq!(
                    reported,
                    Some(expected.as_str()),
                    "case {case:?}/{path}/{agent_type}: expected the inherited binding \
                     {expected:?}, got {reported:?}"
                );
            }
        }
    }
}

/// The revision a service binding names ADDRESSES the built-in agent: the id reported for
/// the type the service binds floating resolves through `GetAIAgent` to that agent's own
/// record, and so does the bare id under it — so a client that follows the reported
/// binding reaches the agent that runs for the type.
#[tokio::test]
async fn the_floating_binding_addresses_the_built_in_agent() {
    let emulator = Emulator::new();
    let assistant_row = AssistantRow {
        label: "floating binding owner",
        region: REGION,
        body: json!({ "name": "floating-binding", "type": "AGENT" }),
    };
    let assistant_id = created_assistant_id(&emulator, &assistant_row).await;

    for (agent_type, ai_agent_id, qualifier) in BUILT_IN_AGENTS {
        if qualifier != QUALIFIER_LATEST {
            // A pinned revision names a published VERSION of the agent, which this
            // emulator publishes none of yet; that read is its own operation's contract.
            continue;
        }
        for addressed_by in [
            ai_agent_id.to_string(),
            expected_binding_id(agent_type),
        ] {
            let read = emulator
                .get_ai_agent(REGION, &assistant_id, &addressed_by)
                .await;
            assert_eq!(
                read.status, 200,
                "case {agent_type:?}: expected HTTP 200 reading the agent addressed by \
                 {addressed_by:?}, got {} with body {}",
                read.status, read.body
            );
            assert_eq!(
                read.string("/aiAgent/aiAgentId").as_deref(),
                Some(ai_agent_id),
                "case {agent_type:?}: expected the read of {addressed_by:?} to report \
                 aiAgentId {ai_agent_id}, got {:?}",
                read.string("/aiAgent/aiAgentId")
            );
            assert_eq!(
                read.string("/aiAgent/type").as_deref(),
                Some(agent_type),
                "case {agent_type:?}: expected the agent addressed by {addressed_by:?} to \
                 serve type {agent_type}, got {:?}",
                read.string("/aiAgent/type")
            );
            assert_eq!(
                read.string("/aiAgent/origin").as_deref(),
                Some(ORIGIN_SYSTEM),
                "case {agent_type:?}: expected origin {ORIGIN_SYSTEM} for an agent the \
                 service provisioned, got {:?}",
                read.string("/aiAgent/origin")
            );
            // The floating alias names the agent itself, so the read reports no version
            // number: there is no published version in play.
            assert!(
                read.field("/versionNumber").is_none(),
                "case {agent_type:?}: expected no versionNumber reading {addressed_by:?}, \
                 got {}",
                read.body
            );
        }
    }
}
