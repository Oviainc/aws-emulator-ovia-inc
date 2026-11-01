// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the defaults the SERVICE provisions an assistant with: the
//! built-in AI agents every assistant and session is bound to, and the system-managed
//! session every assistant owns.
//!
//! Two rules are under test:
//!
//! 1. The built-in AI agents belong to the service, not to the assistant that reports
//!    them: one agent per AI agent type with a built-in agent, named by the service's own
//!    id and the published version of it that runs, so EVERY assistant and EVERY session —
//!    whatever the create asked for, in whatever region — reports the same bindings, and a
//!    second create of either reports them again.
//! 2. Every assistant owns a service-managed session, of `SYSTEM` origin and reachable at
//!    the fixed all-zero `sessionId`, from the moment the assistant exists: a `GetSession`
//!    on it needs no `CreateSession` first, and it reports the assistant's built-in
//!    bindings and only the members it holds.
//!
//! The tests are TABLES: the rows differ along the create input (name form and length,
//! description, tag count, encryption configuration, signing region) and, for the
//! sessions, along what the create configures for the session itself — none of which is
//! what decides a service-provisioned default.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts; nothing reaches into the service's state struct.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region a row signs with unless it is exercising the region axis itself.
const REGION: &str = "us-east-1";

/// The built-in AI agent the service binds for each AI agent type, written out here
/// rather than imported from `state.rs` so the assertions state the values the recorded
/// wire answers of `CreateAssistant`, `CreateSession` and `GetSession` carry: one entry
/// per `AIAgentType` the model declares
/// (`smithy_query(query="shape",shape="AIAgentType")`) except `ORCHESTRATION`, which has
/// no built-in agent, each the service's `<agentId>:<qualifier>` for the type — the
/// floating `$LATEST` alias where the service binds the agent itself, the published
/// version number where it pins one.
const BUILT_IN_AGENTS: [(&str, &str); 8] = [
    ("ANSWER_RECOMMENDATION", "3025a3fa-5419-4c9c-a040-877213b93f70:$LATEST"),
    ("CASE_SUMMARIZATION", "9114812c-2a1e-4501-8817-daec4223ce57:3"),
    ("EMAIL_GENERATIVE_ANSWER", "4d7e9f2a-1b3c-5d6e-8f7a-9b0c1d2e3f4a:7"),
    ("EMAIL_OVERVIEW", "91bb967b-bd5e-4e02-a8d5-cb7c5b2bf9c7:5"),
    ("EMAIL_RESPONSE", "8f4e7d2a-9c31-4b5e-a6f8-e2d9c7b54321:7"),
    ("MANUAL_SEARCH", "652f0287-1610-4fee-bbe3-c687c13f5ec3:9"),
    ("NOTE_TAKING", "7fc84960-5b46-4e44-9dd6-5956b2acfaa4:3"),
    ("SELF_SERVICE", "529207fe-80fd-42be-bcf2-facd2141aec7:5"),
];

/// The one AI agent type whose binding reports `enabled` at all — the model's
/// `AIAgentConfigurationData$enabled` doc says disabling "is currently supported only for
/// the `ANSWER_RECOMMENDATION` AI Agent type" — and whose built-in binding is reported
/// disabled.
const ANSWER_RECOMMENDATION: &str = "ANSWER_RECOMMENDATION";

/// The qualifier that names an AI agent's floating alias rather than one of its published
/// versions, which is what the service's binding to the answer-recommendation default
/// carries.
const QUALIFIER_LATEST: &str = "$LATEST";

/// The `AIAgentType` the service publishes no built-in agent for, so no fresh assistant
/// and no fresh session reports a binding for it.
const UNBOUND_AI_AGENT_TYPE: &str = "ORCHESTRATION";

/// The `sessionId` of the session the service manages for every assistant, and the fixed
/// text it reports under `name` and `description`.
const SYSTEM_SESSION_ID: &str = "00000000-0000-0000-0000-000000000000";
const SYSTEM_SESSION_NAME: &str = "QinConnectSystemSession";
const SYSTEM_SESSION_DESCRIPTION: &str = "System-managed session for Amazon Q in Connect.";

/// The `Origin` values the model declares (`smithy_query(query="shape",shape="Origin")`):
/// `SYSTEM` for the session Amazon Q in Connect created, `CUSTOMER` for one a
/// `CreateSession` call created.
const ORIGIN_SYSTEM: &str = "SYSTEM";
const ORIGIN_CUSTOMER: &str = "CUSTOMER";

/// The fixed sentence a request addressed to a session no assistant holds is refused
/// with.
const MESSAGE_SESSION_NOT_FOUND: &str = "Session does not exist.";

/// An id a caller may configure for a type, matching the modelled `UuidWithQualifier`
/// pattern.
const CALLER_AGENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// One wire answer, reduced to what a client reads.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
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
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .map(|value| value.to_str().unwrap_or_default().to_string())
            .unwrap_or_default();
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

    async fn get_assistant(&self, region: &str, assistant_id: &str) -> Reply {
        self.send("GET", &format!("/assistants/{assistant_id}"), region, None)
            .await
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
}

/// The `aiAgentConfiguration` map a fresh assistant and a fresh session report: every
/// built-in agent of [`BUILT_IN_AGENTS`], with `enabled` reported only for the one type
/// whose binding can carry it.
fn expected_built_in_bindings() -> Map<String, Value> {
    let mut bindings = Map::new();
    for (agent_type, ai_agent_id) in BUILT_IN_AGENTS {
        let mut binding = Map::new();
        binding.insert("aiAgentId".to_string(), json!(ai_agent_id));
        if agent_type == ANSWER_RECOMMENDATION {
            binding.insert("enabled".to_string(), json!(false));
        }
        bindings.insert(agent_type.to_string(), Value::Object(binding));
    }
    bindings
}

/// The ARN of a session, written out here rather than taken from the handler's helper so
/// the test pins the format:
/// `arn:aws:wisdom:<region>:<account>:session/<assistantId>/<sessionId>`.
fn expected_session_arn(region: &str, assistant_id: &str, session_id: &str) -> String {
    format!(
        "arn:aws:wisdom:{region}:{account}:session/{assistant_id}/{session_id}",
        account = engine::EMULATOR_ACCOUNT_ID
    )
}

/// One row of the assistant table: the create input differs, what the service provisions
/// does not.
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
            body: json!({ "name": "defaults-minimal", "type": "AGENT" }),
        },
        AssistantRow {
            label: "described, three tags",
            region: REGION,
            body: json!({
                "name": "defaults.described_1",
                "type": "AGENT",
                "description": "Defaults, described. -",
                "tags": { "owner": "qconnect-test", "a.b": "beta", "cost-centre": "42" },
            }),
        },
        AssistantRow {
            label: "shortest legal name, another region",
            region: "eu-west-3",
            body: json!({ "name": "d", "type": "AGENT" }),
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

/// Create one assistant and hand back its id, failing the case if the create did not.
async fn created_assistant_id(emulator: &Emulator, row: &AssistantRow) -> String {
    let created = emulator.create_assistant(row.region, row.body.clone()).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected HTTP 200 from the create, got {} with body {}",
        row.label, created.status, created.body
    );
    let bindings = created.object("/assistant/aiAgentConfiguration");
    assert_eq!(
        bindings.as_ref(),
        Some(&expected_built_in_bindings()),
        "case {:?}: expected the create to report the service's built-in agents {:?}, \
         got {bindings:?}",
        row.label,
        expected_built_in_bindings()
    );
    created
        .string("/assistant/assistantId")
        .unwrap_or_else(|| panic!("case {:?}: no assistantId in {}", row.label, created.body))
}

/// Rule 1: the built-in bindings are service constants — the same ids and published
/// versions for every assistant, whatever the create asked for and whatever region it was
/// signed for, and the same on every later read of the assistant.
#[tokio::test]
async fn built_in_ai_agent_bindings_are_service_constants_for_every_assistant() {
    let emulator = Emulator::new();
    let expected = expected_built_in_bindings();

    for row in assistant_rows() {
        let case = row.label;
        let assistant_id = created_assistant_id(&emulator, &row).await;

        let read = emulator.get_assistant(row.region, &assistant_id).await;
        let read_bindings = read.object("/assistant/aiAgentConfiguration");
        assert_eq!(
            read_bindings.as_ref(),
            Some(&expected),
            "case {case:?}: expected the read to report the same built-in agents \
             {expected:?}, got {read_bindings:?}"
        );
        assert!(
            read.field(&format!(
                "/assistant/aiAgentConfiguration/{UNBOUND_AI_AGENT_TYPE}"
            ))
            .is_none(),
            "case {case:?}: expected no binding for {UNBOUND_AI_AGENT_TYPE}, got {}",
            read.body
        );
    }

    // Every type's binding names its own agent, and the qualifier is the revision of that
    // agent the service bound — the floating alias, or a published version number within
    // the modelled `Version` range (`smithy_query(query="shape",shape="Version")`:
    // `range 1..`) — so no two types share an agent and nothing is bound to a
    // qualifier-less id.
    let mut agents: BTreeSet<&str> = BTreeSet::new();
    for (agent_type, ai_agent_id) in BUILT_IN_AGENTS {
        let (uuid, qualifier) = ai_agent_id
            .split_once(':')
            .unwrap_or_else(|| panic!("case {agent_type:?}: {ai_agent_id} carries no qualifier"));
        assert!(
            agents.insert(uuid),
            "case {agent_type:?}: agent {uuid} is already bound to another type"
        );
        if agent_type == ANSWER_RECOMMENDATION {
            assert_eq!(
                qualifier, QUALIFIER_LATEST,
                "case {agent_type:?}: expected the floating qualifier {QUALIFIER_LATEST}, \
                 got {qualifier:?}"
            );
        } else {
            assert!(
                qualifier.parse::<i64>().is_ok_and(|version| version >= 1),
                "case {agent_type:?}: expected a published version of at least 1, got \
                 {qualifier:?}"
            );
        }
    }
}

/// Rule 1, continued: a session inherits the assistant's built-in bindings, reports them
/// on the create's own answer and on every later read, and keeps them for the types the
/// create did not configure itself.
#[tokio::test]
async fn a_session_inherits_the_built_in_bindings_it_does_not_override() {
    let emulator = Emulator::new();
    let inherited = expected_built_in_bindings();

    for row in assistant_rows() {
        let case = row.label;
        let assistant_id = created_assistant_id(&emulator, &row).await;

        // A session that configures nothing reports exactly the inherited bindings.
        let created = emulator
            .create_session(row.region, &assistant_id, json!({ "name": "inherits" }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the session create, got {} with body {}",
            created.status, created.body
        );
        let created_bindings = created.object("/session/aiAgentConfiguration");
        assert_eq!(
            created_bindings.as_ref(),
            Some(&inherited),
            "case {case:?}: expected the session to inherit {inherited:?}, \
             got {created_bindings:?}"
        );
        let session_id = created
            .string("/session/sessionId")
            .unwrap_or_else(|| panic!("case {case:?}: no sessionId in {}", created.body));
        let read = emulator
            .get_session(row.region, &assistant_id, &session_id)
            .await;
        let read_bindings = read.object("/session/aiAgentConfiguration");
        assert_eq!(
            read_bindings.as_ref(),
            Some(&inherited),
            "case {case:?}: expected the read of the session to report {inherited:?}, \
             got {read_bindings:?}"
        );
        assert_eq!(
            read.string("/session/origin").as_deref(),
            Some(ORIGIN_CUSTOMER),
            "case {case:?}: expected a created session to report origin \
             {ORIGIN_CUSTOMER}, got {:?}",
            read.string("/session/origin")
        );

        // One type configured by the caller is the caller's; every other type is still
        // the service's built-in agent.
        let mut expected_overridden = inherited.clone();
        expected_overridden.insert(
            "MANUAL_SEARCH".to_string(),
            json!({ "aiAgentId": CALLER_AGENT_ID }),
        );
        let overridden = emulator
            .create_session(
                row.region,
                &assistant_id,
                json!({
                    "name": "overrides-one-type",
                    "aiAgentConfiguration": {
                        "MANUAL_SEARCH": { "aiAgentId": CALLER_AGENT_ID }
                    },
                }),
            )
            .await;
        let overridden_bindings = overridden.object("/session/aiAgentConfiguration");
        assert_eq!(
            overridden_bindings.as_ref(),
            Some(&expected_overridden),
            "case {case:?}: expected the configured type to be the caller's and the rest \
             the service's ({expected_overridden:?}), got {overridden_bindings:?}"
        );
    }
}

/// Rule 2: the service-managed session is readable at the all-zero `sessionId` right
/// after the assistant was created, with no `CreateSession` in between, and reports the
/// fixed text the service gives it, its own assistant's ARN and the built-in bindings.
#[tokio::test]
async fn the_system_managed_session_is_readable_as_soon_as_the_assistant_exists() {
    let emulator = Emulator::new();
    let expected_bindings = expected_built_in_bindings();
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for row in assistant_rows() {
        let case = row.label;
        let assistant_id = created_assistant_id(&emulator, &row).await;

        let read = emulator
            .get_session(row.region, &assistant_id, SYSTEM_SESSION_ID)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 for the system session, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("/session/sessionId").as_deref(),
            Some(SYSTEM_SESSION_ID),
            "case {case:?}: expected sessionId {SYSTEM_SESSION_ID}, got {:?}",
            read.string("/session/sessionId")
        );
        assert_eq!(
            read.string("/session/name").as_deref(),
            Some(SYSTEM_SESSION_NAME),
            "case {case:?}: expected name {SYSTEM_SESSION_NAME}, got {:?}",
            read.string("/session/name")
        );
        assert_eq!(
            read.string("/session/description").as_deref(),
            Some(SYSTEM_SESSION_DESCRIPTION),
            "case {case:?}: expected description {SYSTEM_SESSION_DESCRIPTION:?}, got {:?}",
            read.string("/session/description")
        );
        assert_eq!(
            read.string("/session/origin").as_deref(),
            Some(ORIGIN_SYSTEM),
            "case {case:?}: expected origin {ORIGIN_SYSTEM}, got {:?}",
            read.string("/session/origin")
        );
        let expected_arn = expected_session_arn(row.region, &assistant_id, SYSTEM_SESSION_ID);
        assert_eq!(
            read.string("/session/sessionArn"),
            Some(expected_arn.clone()),
            "case {case:?}: expected sessionArn {expected_arn}, got {:?}",
            read.string("/session/sessionArn")
        );
        assert!(
            arns.insert(expected_arn.clone()),
            "case {case:?}: ARN {expected_arn} was already another assistant's system \
             session"
        );
        let bindings = read.object("/session/aiAgentConfiguration");
        assert_eq!(
            bindings.as_ref(),
            Some(&expected_bindings),
            "case {case:?}: expected the system session to report the assistant's \
             built-in agents {expected_bindings:?}, got {bindings:?}"
        );
        // It holds nothing a caller configured, so it reports no member for one.
        for absent in [
            "/session/integrationConfiguration",
            "/session/orchestratorConfigurationList",
            "/session/tagFilter",
        ] {
            assert!(
                read.field(absent).is_none(),
                "case {case:?}: expected no {absent} on a session no caller configured, \
                 got {}",
                read.body
            );
        }

        // A read reports; it does not change what the next caller sees.
        let again = emulator
            .get_session(row.region, &assistant_id, SYSTEM_SESSION_ID)
            .await;
        assert_eq!(
            again.body, read.body,
            "case {case:?}: two reads of the system session disagreed: {} vs {}",
            again.body, read.body
        );
    }
}

/// Rule 2, continued: it is the all-zero id that names the service-managed session —
/// every other well-formed session id the assistant holds nothing under is still the
/// modelled 404.
#[tokio::test]
async fn only_the_all_zero_session_id_names_the_system_session() {
    let emulator = Emulator::new();
    let assistant_id = created_assistant_id(
        &emulator,
        &AssistantRow {
            label: "system session owner",
            region: REGION,
            body: json!({ "name": "only-the-zero-id", "type": "AGENT" }),
        },
    )
    .await;

    // Rows differ in how far from the all-zero id they are: one digit, the last digit,
    // the all-f id and an unrelated well-formed uuid.
    let cases = [
        ("one digit past the zero id", "00000000-0000-0000-0000-000000000001"),
        ("the zero id with a changed first digit", "10000000-0000-0000-0000-000000000000"),
        ("every digit set", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ("an unrelated well-formed uuid", "1b4e28ba-2fa1-11d2-883f-0016d3cca427"),
    ];
    for (case, session_id) in cases {
        let refused = emulator
            .get_session(REGION, &assistant_id, session_id)
            .await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for {session_id}, got {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?} with body {}",
            refused.error_type,
            refused.body
        );
        assert_eq!(
            refused.string("/message").as_deref(),
            Some(MESSAGE_SESSION_NOT_FOUND),
            "case {case:?}: expected the sentence {MESSAGE_SESSION_NOT_FOUND:?}, got {:?}",
            refused.string("/message")
        );
    }
}
