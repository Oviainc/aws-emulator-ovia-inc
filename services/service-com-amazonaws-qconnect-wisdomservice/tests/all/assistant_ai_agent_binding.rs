// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AI agent an assistant is CONFIGURED with:
//! `UpdateAssistantAIAgent` and `RemoveAssistantAIAgent`, read back through
//! `GetAssistant` and `ListAssistants`.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one recorded
//! request: an assistant's `aiAgentConfiguration` reports, per `AIAgentType`, the binding a
//! caller configured for that type and the service's own default binding for every type no
//! caller has configured — so a configure is reported by every read of the assistant, byte
//! for byte as it was sent, until it is removed, and a removal puts the type back on the
//! service default (on nothing, for a type the service publishes no default for).
//!
//! The rows differ along every axis the rule generalises over:
//!
//! * every `AIAgentType` the model declares
//!   (`smithy_query(query="shape",shape="AIAgentType")`), including the one with no
//!   service default;
//! * how the binding NAMES its agent — a bare uuid, or the `$LATEST`-qualified form the
//!   modelled `UuidWithQualifier` admits;
//! * whether the binding carries `enabled`, and with which value;
//! * whether the request names an orchestrator use case, and how many use cases the
//!   assistant ends up carrying.
//!
//! Beside the round trip it pins the refusals: an assistant this account does not hold, a
//! binding naming no AI agent of that assistant, one naming an agent of ANOTHER assistant,
//! a numbered version qualifier (nothing publishes a version yet), an agent whose own type
//! is not the type being configured, and a binding turned off for a type whose binding
//! cannot be turned off.
//!
//! Every assertion goes over the wire — only the HTTP status, the `x-amzn-errortype`
//! header and the JSON body are inspected — so a response that is not computed from
//! persisted state cannot pass. The harness (the router, the reply reader and the creates)
//! is the one `resource_tags.rs` and `ai_agent.rs` already drive this service with.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::ai_agent::{
    create_agent, full_configuration, MESSAGE_AI_AGENT_NOT_FOUND, MESSAGE_ASSISTANT_NOT_FOUND,
    QUALIFIER_LATEST, VISIBILITY_PUBLISHED,
};
use service_com_amazonaws_qconnect_wisdomservice::state::MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT;

use super::resource_tags::{label, Emulator, Reply, ABSENT_UUID};

/// Every value the modelled `AIAgentType` declares
/// (`smithy_query(query="shape",shape="AIAgentType")`), written out here rather than
/// imported from the implementation so the table spans the whole catalog.
const AI_AGENT_TYPES: [&str; 9] = [
    "MANUAL_SEARCH",
    "ANSWER_RECOMMENDATION",
    "SELF_SERVICE",
    "EMAIL_RESPONSE",
    "EMAIL_OVERVIEW",
    "EMAIL_GENERATIVE_ANSWER",
    "ORCHESTRATION",
    "NOTE_TAKING",
    "CASE_SUMMARIZATION",
];

/// The one type whose binding the model lets a caller turn off: the
/// `AIAgentConfigurationData$enabled` doc says "Setting this value to false is currently
/// supported only for the `ANSWER_RECOMMENDATION` AI Agent type; other requests to set it
/// to false are rejected with a validation error".
const ANSWER_RECOMMENDATION: &str = "ANSWER_RECOMMENDATION";

/// The one type a fresh assistant reports no default binding for, which is the type an
/// orchestrator use case configures.
const ORCHESTRATION: &str = "ORCHESTRATION";

/// Two orchestrator use cases, worded as the model's own
/// `OrchestratorConfigurationEntry$orchestratorUseCase` doc words them ("for example
/// Connect.SelfService, Connect.AgentAssistance").
const USE_CASE_SELF_SERVICE: &str = "Connect.SelfService";
const USE_CASE_AGENT_ASSISTANCE: &str = "Connect.AgentAssistance";

/// The modelled error shapes this surface refuses with, as the wire reports them.
const CODE_NOT_FOUND: &str = "ResourceNotFoundException";
const CODE_VALIDATION: &str = "ValidationException";

/// The sentences this endpoint refuses a binding its own constraints reject with. Written
/// out here rather than imported from the implementation, so the assertions pin the answer
/// a client sees.
fn message_type_mismatch(agent_type: &str) -> String {
    format!("The AI Agent configured for AI Agent type {agent_type} must be an AI Agent of that type.")
}
const MESSAGE_NOT_DISABLEABLE: &str =
    "Setting enabled to false is supported only for the ANSWER_RECOMMENDATION AI Agent type.";

/// The sentence a configure past the store's ceiling on orchestrator use cases is refused
/// with. The ceiling itself is imported, because the figure is this emulator's own heap
/// guard rather than a published quota; the sentence is written out here, because it is
/// what a client sees.
fn message_orchestrator_quota() -> String {
    format!(
        "The maximum number of orchestrator configurations \
         ({MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT}) in this assistant has been \
         reached. Remove an orchestrator configuration before adding another."
    )
}

/// `UpdateAssistantAIAgent`, as the model's URI binds it
/// (`POST /assistants/{assistantId}/aiagentConfiguration`).
async fn configure(emulator: &Emulator, assistant: &str, body: Value) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/assistants/{}/aiagentConfiguration", label(assistant)),
            Some(body),
        )
        .await
}

/// `RemoveAssistantAIAgent` (`DELETE /assistants/{assistantId}/aiagentConfiguration`,
/// whose `aiAgentType` and `orchestratorUseCase` are `@httpQuery` members).
async fn remove_configured(
    emulator: &Emulator,
    assistant: &str,
    agent_type: &str,
    orchestrator_use_case: Option<&str>,
) -> Reply {
    let mut uri = format!(
        "/assistants/{}/aiagentConfiguration?aiAgentType={agent_type}",
        label(assistant)
    );
    if let Some(use_case) = orchestrator_use_case {
        uri.push_str(&format!("&orchestratorUseCase={}", label(use_case)));
    }
    emulator.send("DELETE", &uri, None).await
}

/// `GetAssistant` (`GET /assistants/{assistantId}`).
async fn get_assistant(emulator: &Emulator, assistant: &str) -> Reply {
    emulator
        .send("GET", &format!("/assistants/{}", label(assistant)), None)
        .await
}

/// `ListAssistants` (`GET /assistants`).
async fn list_assistants(emulator: &Emulator) -> Reply {
    emulator.send("GET", "/assistants", None).await
}

/// A fresh assistant, answering with its id and the bindings its create provisioned — the
/// service defaults every assertion below measures a configure against.
async fn fresh_assistant(emulator: &Emulator, name: &str) -> (String, Value) {
    let created = emulator.create_assistant(name, &BTreeMap::new()).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the assistant this suite configures must be created, got HTTP {} \
         with body {}",
        created.status, created.body
    );
    let id = created.string("/assistant/assistantId");
    let defaults = created
        .field("/assistant/aiAgentConfiguration")
        .cloned()
        .unwrap_or_else(|| panic!("case {name:?}: the create reported no aiAgentConfiguration"));
    (id, defaults)
}

/// An AI agent of `agent_type` under `assistant`, answering with its minted id.
async fn agent_of_type(emulator: &Emulator, assistant: &str, agent_type: &str, name: &str) -> String {
    let created = create_agent(
        emulator,
        assistant,
        json!({
            "name": name,
            "type": agent_type,
            "visibilityStatus": VISIBILITY_PUBLISHED,
            "configuration": full_configuration(agent_type),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the agent this row binds must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    created.string("/aiAgent/aiAgentId")
}

/// The `aiAgentConfiguration` one reported assistant carries, at the pointer prefix of the
/// assistant object (`/assistant` for a read, an indexed summary for the list).
fn bindings(reply: &Reply, prefix: &str) -> Value {
    reply
        .field(&format!("{prefix}/aiAgentConfiguration"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `orchestratorConfigurationList` one reported assistant carries.
fn orchestrators(reply: &Reply, prefix: &str) -> Value {
    reply
        .body
        .pointer(&format!("{prefix}/orchestratorConfigurationList"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// One row of the configure table: which type is configured, how the binding names its
/// agent, and what `enabled` it carries.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    agent_type: &'static str,
    /// The version qualifier the binding appends to the agent's id, or `None` for the bare
    /// uuid form.
    qualifier: Option<&'static str>,
    /// The `enabled` member the binding carries, or `None` when it carries none.
    enabled: Option<bool>,
}

fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = AI_AGENT_TYPES
        .iter()
        .map(|agent_type| Row {
            label: "a bare agent id for one type",
            agent_type,
            qualifier: None,
            enabled: None,
        })
        .collect();
    // The qualified form the modelled `UuidWithQualifier` admits: a caller that pinned the
    // agent's working copy keeps exactly the id it pinned.
    rows.push(Row {
        label: "a $LATEST-qualified agent id",
        agent_type: "MANUAL_SEARCH",
        qualifier: Some(QUALIFIER_LATEST),
        enabled: None,
    });
    rows.push(Row {
        label: "a $LATEST-qualified agent id for the type with no default",
        agent_type: ORCHESTRATION,
        qualifier: Some(QUALIFIER_LATEST),
        enabled: None,
    });
    // `enabled` sent explicitly: true for a type that can carry only that, and both values
    // for the one type the model lets a caller turn off.
    rows.push(Row {
        label: "enabled=true on a type that cannot be turned off",
        agent_type: "NOTE_TAKING",
        qualifier: None,
        enabled: Some(true),
    });
    rows.push(Row {
        label: "enabled=true on the type that can be turned off",
        agent_type: ANSWER_RECOMMENDATION,
        qualifier: None,
        enabled: Some(true),
    });
    rows.push(Row {
        label: "enabled=false on the one type that can be turned off",
        agent_type: ANSWER_RECOMMENDATION,
        qualifier: None,
        enabled: Some(false),
    });
    rows
}

/// Configuring an AI agent for a type is reported by every read of the assistant, for
/// every row — and removing it puts the type back on what the service provisioned.
#[tokio::test]
async fn a_configured_ai_agent_is_what_every_read_of_the_assistant_reports() {
    for row in rows() {
        let case = format!("{} ({})", row.label, row.agent_type);
        // One emulator per row, so the list below reports exactly this row's assistant.
        let emulator = Emulator::new();
        let (assistant, defaults) = fresh_assistant(&emulator, &case).await;
        let agent = agent_of_type(&emulator, &assistant, row.agent_type, &case).await;

        let configured_id = match row.qualifier {
            Some(qualifier) => format!("{agent}:{qualifier}"),
            None => agent.clone(),
        };
        let mut binding = json!({ "aiAgentId": configured_id });
        if let Some(enabled) = row.enabled {
            binding["enabled"] = json!(enabled);
        }
        // What every read must report once this request is served: the service's defaults,
        // with this row's type bound to what the request carried.
        let mut expected = defaults
            .as_object()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the create reported no bindings"));
        expected.insert(row.agent_type.to_string(), binding.clone());
        let expected = Value::Object(expected);

        let updated = configure(
            &emulator,
            &assistant,
            json!({ "aiAgentType": row.agent_type, "configuration": binding }),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            updated.status, updated.body
        );
        let answered = bindings(&updated, "/assistant");
        assert_eq!(
            answered, expected,
            "case {case:?}: expected the configure to answer {expected}, got {answered}"
        );

        let read = get_assistant(&emulator, &assistant).await;
        let after_read = bindings(&read, "/assistant");
        assert_eq!(
            after_read, expected,
            "case {case:?}: expected GetAssistant to report {expected}, got {after_read}"
        );
        // A read must not change what the next caller sees.
        let again = get_assistant(&emulator, &assistant).await;
        assert_eq!(
            bindings(&again, "/assistant"),
            expected,
            "case {case:?}: two reads of one assistant disagreed"
        );

        let listed = list_assistants(&emulator).await;
        let summarised = bindings(&listed, "/assistantSummaries/0");
        assert_eq!(
            summarised, expected,
            "case {case:?}: expected the ListAssistants summary to report {expected}, \
             got {summarised}"
        );
        // Configuring a type per se configures no orchestrator use case.
        let listed_orchestrators = orchestrators(&read, "/assistant");
        assert_eq!(
            listed_orchestrators,
            json!([]),
            "case {case:?}: expected no orchestrator entry, got {listed_orchestrators}"
        );

        // Removing the configured agent puts the type back where it started: on the
        // service's own default, and on no binding at all for the type that has none.
        let removed = remove_configured(&emulator, &assistant, row.agent_type, None).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: expected HTTP 204 from the removal, got {} with body {}",
            removed.status, removed.body
        );
        // A 204 carries no members: the framework renders this service's empty
        // `RemoveAssistantAIAgentResponse` as an empty JSON object.
        let members = match &removed.body {
            Value::Null => 0,
            Value::Object(members) => members.len(),
            other => panic!("case {case:?}: expected an empty body, got {other}"),
        };
        assert_eq!(
            members, 0,
            "case {case:?}: expected a body with no members, got {}",
            removed.body
        );
        let after_removal = get_assistant(&emulator, &assistant).await;
        assert_eq!(
            bindings(&after_removal, "/assistant"),
            defaults,
            "case {case:?}: expected the service defaults back after the removal, got {}",
            bindings(&after_removal, "/assistant")
        );
        // The operation is modelled `@idempotent`: removing again is the same success and
        // reports the same configuration.
        let removed_again = remove_configured(&emulator, &assistant, row.agent_type, None).await;
        assert_eq!(
            removed_again.status, 204,
            "case {case:?}: expected HTTP 204 from the second removal, got {} with body {}",
            removed_again.status, removed_again.body
        );
        let after_second = get_assistant(&emulator, &assistant).await;
        assert_eq!(
            bindings(&after_second, "/assistant"),
            defaults,
            "case {case:?}: a second removal changed the reported configuration"
        );
        // The agent itself is not what a removal removes: it is still readable.
        let agent_read = super::ai_agent::get_agent(&emulator, &assistant, &agent).await;
        assert_eq!(
            agent_read.status, 200,
            "case {case:?}: expected the AI agent to outlive the binding, got HTTP {} \
             with body {}",
            agent_read.status, agent_read.body
        );
    }
}

/// One row of the refusal table: the request, and the answer it must be refused with.
#[derive(Debug, Clone)]
struct Refusal {
    label: &'static str,
    /// The assistant label the request addresses, or `None` for the row's own assistant.
    assistant: Option<String>,
    agent_type: &'static str,
    /// The `aiAgentId` the binding carries; `None` means the row's own agent id.
    ai_agent_id: Option<String>,
    enabled: Option<bool>,
    status: u16,
    code: &'static str,
    message: String,
}

/// Every refusal this endpoint words, each with the stored configuration left untouched.
#[tokio::test]
async fn a_binding_the_service_cannot_configure_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let (assistant, defaults) = fresh_assistant(&emulator, "refusals").await;
    let manual_search = agent_of_type(&emulator, &assistant, "MANUAL_SEARCH", "refusals").await;
    // An agent of ANOTHER assistant: an AI agent is reachable only through its own parent.
    let (other_assistant, _) = fresh_assistant(&emulator, "refusals: other assistant").await;
    let foreign = agent_of_type(
        &emulator,
        &other_assistant,
        "MANUAL_SEARCH",
        "refusals: other assistant",
    )
    .await;

    let refusals = vec![
        Refusal {
            label: "an assistant this account does not hold",
            assistant: Some(ABSENT_UUID.to_string()),
            agent_type: "MANUAL_SEARCH",
            ai_agent_id: None,
            enabled: None,
            status: 404,
            code: CODE_NOT_FOUND,
            message: MESSAGE_ASSISTANT_NOT_FOUND.to_string(),
        },
        Refusal {
            label: "a binding naming no AI agent of this assistant",
            assistant: None,
            agent_type: "MANUAL_SEARCH",
            ai_agent_id: Some(ABSENT_UUID.to_string()),
            enabled: None,
            status: 404,
            code: CODE_NOT_FOUND,
            message: MESSAGE_AI_AGENT_NOT_FOUND.to_string(),
        },
        Refusal {
            label: "a binding naming an agent of another assistant",
            assistant: None,
            agent_type: "MANUAL_SEARCH",
            ai_agent_id: Some(foreign.clone()),
            enabled: None,
            status: 404,
            code: CODE_NOT_FOUND,
            message: MESSAGE_AI_AGENT_NOT_FOUND.to_string(),
        },
        Refusal {
            label: "a numbered version qualifier, which nothing publishes yet",
            assistant: None,
            agent_type: "MANUAL_SEARCH",
            ai_agent_id: Some(format!("{manual_search}:1")),
            enabled: None,
            status: 404,
            code: CODE_NOT_FOUND,
            message: MESSAGE_AI_AGENT_NOT_FOUND.to_string(),
        },
        Refusal {
            label: "an agent whose own type is not the type being configured",
            assistant: None,
            agent_type: "NOTE_TAKING",
            ai_agent_id: None,
            enabled: None,
            status: 400,
            code: CODE_VALIDATION,
            message: message_type_mismatch("NOTE_TAKING"),
        },
        Refusal {
            label: "a binding turned off for a type whose binding cannot be turned off",
            assistant: None,
            agent_type: "MANUAL_SEARCH",
            ai_agent_id: None,
            enabled: Some(false),
            status: 400,
            code: CODE_VALIDATION,
            message: MESSAGE_NOT_DISABLEABLE.to_string(),
        },
    ];

    for refusal in refusals {
        let case = refusal.label;
        let mut binding = json!({
            "aiAgentId": refusal.ai_agent_id.clone().unwrap_or_else(|| manual_search.clone()),
        });
        if let Some(enabled) = refusal.enabled {
            binding["enabled"] = json!(enabled);
        }
        let addressed = refusal.assistant.clone().unwrap_or_else(|| assistant.clone());
        let reply = configure(
            &emulator,
            &addressed,
            json!({ "aiAgentType": refusal.agent_type, "configuration": binding }),
        )
        .await;
        assert_eq!(
            reply.status, refusal.status,
            "case {case:?}: expected HTTP {}, got {} with body {}",
            refusal.status, reply.status, reply.body
        );
        assert_eq!(
            reply.error_type, refusal.code,
            "case {case:?}: expected error code {}, got {:?}",
            refusal.code, reply.error_type
        );
        let message = reply.string("/message");
        assert_eq!(
            message, refusal.message,
            "case {case:?}: expected message {:?}, got {message:?}",
            refusal.message
        );
        // A refusal writes nothing: the assistant still reports what it was provisioned
        // with.
        let read = get_assistant(&emulator, &assistant).await;
        assert_eq!(
            bindings(&read, "/assistant"),
            defaults,
            "case {case:?}: a refused configure changed the stored configuration"
        );
    }

    // A removal addressed at an assistant this account does not hold is the same
    // not-found, and the idempotent removal of a type never configured is not one.
    let removal = remove_configured(&emulator, ABSENT_UUID, "MANUAL_SEARCH", None).await;
    assert_eq!(
        removal.status, 404,
        "a removal on an absent assistant must be refused, got HTTP {} with body {}",
        removal.status, removal.body
    );
    assert_eq!(
        removal.error_type, CODE_NOT_FOUND,
        "a removal on an absent assistant must carry {CODE_NOT_FOUND}, got {:?}",
        removal.error_type
    );
    assert_eq!(
        removal.string("/message"),
        MESSAGE_ASSISTANT_NOT_FOUND,
        "a removal on an absent assistant must state the service's sentence, got {:?}",
        removal.string("/message")
    );

    // `enabled=false` IS what the one type that can be turned off accepts, so the refusal
    // above is about the type and not about the member.
    let answer_recommendation =
        agent_of_type(&emulator, &assistant, ANSWER_RECOMMENDATION, "disable").await;
    let accepted = configure(
        &emulator,
        &assistant,
        json!({
            "aiAgentType": ANSWER_RECOMMENDATION,
            "configuration": { "aiAgentId": answer_recommendation, "enabled": false },
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "turning the one disableable type off must be accepted, got HTTP {} with body {}",
        accepted.status, accepted.body
    );
}

/// A request naming an orchestrator use case configures that use case's entry of the
/// assistant's orchestrator list, which is the list `GetAssistant` and `ListAssistants`
/// report — one entry per use case, replaced rather than duplicated, and removed on its own.
#[tokio::test]
async fn an_orchestrator_use_case_is_configured_per_use_case() {
    let emulator = Emulator::new();
    let (assistant, defaults) = fresh_assistant(&emulator, "orchestrators").await;
    let first = agent_of_type(&emulator, &assistant, ORCHESTRATION, "orchestrator one").await;
    let second = agent_of_type(&emulator, &assistant, ORCHESTRATION, "orchestrator two").await;

    let entry = |agent: &str, use_case: &str| {
        json!({ "aiAgentId": agent, "orchestratorUseCase": use_case })
    };
    let configure_use_case = |agent: String, use_case: &'static str| {
        let assistant = assistant.clone();
        let emulator = &emulator;
        async move {
            configure(
                emulator,
                &assistant,
                json!({
                    "aiAgentType": ORCHESTRATION,
                    "configuration": { "aiAgentId": agent },
                    "orchestratorUseCase": use_case,
                }),
            )
            .await
        }
    };

    // The first use case: one entry, and the per-type binding map untouched — an
    // orchestrator agent is configured per use case, not per AI agent type.
    let reply = configure_use_case(first.clone(), USE_CASE_SELF_SERVICE).await;
    assert_eq!(
        reply.status, 200,
        "case {USE_CASE_SELF_SERVICE:?}: expected HTTP 200, got {} with body {}",
        reply.status, reply.body
    );
    let expected = json!([entry(&first, USE_CASE_SELF_SERVICE)]);
    assert_eq!(
        orchestrators(&reply, "/assistant"),
        expected,
        "case {USE_CASE_SELF_SERVICE:?}: expected {expected}, got {}",
        orchestrators(&reply, "/assistant")
    );
    assert_eq!(
        bindings(&reply, "/assistant"),
        defaults,
        "case {USE_CASE_SELF_SERVICE:?}: a use case must configure no AI agent type, got {}",
        bindings(&reply, "/assistant")
    );

    // The SAME use case configured again names one agent, so the entry is replaced.
    let reply = configure_use_case(second.clone(), USE_CASE_SELF_SERVICE).await;
    let expected = json!([entry(&second, USE_CASE_SELF_SERVICE)]);
    assert_eq!(
        orchestrators(&reply, "/assistant"),
        expected,
        "case {USE_CASE_SELF_SERVICE:?} reconfigured: expected {expected}, got {}",
        orchestrators(&reply, "/assistant")
    );

    // A SECOND use case is its own entry, and both are reported by the reads.
    let reply = configure_use_case(first.clone(), USE_CASE_AGENT_ASSISTANCE).await;
    let expected = json!([
        entry(&second, USE_CASE_SELF_SERVICE),
        entry(&first, USE_CASE_AGENT_ASSISTANCE),
    ]);
    assert_eq!(
        orchestrators(&reply, "/assistant"),
        expected,
        "case {USE_CASE_AGENT_ASSISTANCE:?}: expected {expected}, got {}",
        orchestrators(&reply, "/assistant")
    );
    let read = get_assistant(&emulator, &assistant).await;
    assert_eq!(
        orchestrators(&read, "/assistant"),
        expected,
        "GetAssistant must report the configured orchestrator entries, got {}",
        orchestrators(&read, "/assistant")
    );
    let listed = list_assistants(&emulator).await;
    assert_eq!(
        orchestrators(&listed, "/assistantSummaries/0"),
        expected,
        "the ListAssistants summary must report the same entries, got {}",
        orchestrators(&listed, "/assistantSummaries/0")
    );

    // Removing one use case leaves the other where it was.
    let removed = remove_configured(
        &emulator,
        &assistant,
        ORCHESTRATION,
        Some(USE_CASE_SELF_SERVICE),
    )
    .await;
    assert_eq!(
        removed.status, 204,
        "expected HTTP 204 from the removal, got {} with body {}",
        removed.status, removed.body
    );
    let read = get_assistant(&emulator, &assistant).await;
    let expected = json!([entry(&first, USE_CASE_AGENT_ASSISTANCE)]);
    assert_eq!(
        orchestrators(&read, "/assistant"),
        expected,
        "expected only the use case that was not removed, got {}",
        orchestrators(&read, "/assistant")
    );
    let removed = remove_configured(
        &emulator,
        &assistant,
        ORCHESTRATION,
        Some(USE_CASE_AGENT_ASSISTANCE),
    )
    .await;
    assert_eq!(
        removed.status, 204,
        "expected HTTP 204 from the second removal, got {} with body {}",
        removed.status, removed.body
    );
    let read = get_assistant(&emulator, &assistant).await;
    assert_eq!(
        orchestrators(&read, "/assistant"),
        json!([]),
        "expected an empty orchestrator list once every use case is removed, got {}",
        orchestrators(&read, "/assistant")
    );
    assert_eq!(
        bindings(&read, "/assistant"),
        defaults,
        "no orchestrator use case may change what the AI agent types report, got {}",
        bindings(&read, "/assistant")
    );
}

/// The store's ceiling on the orchestrator use cases one assistant carries: exactly at it
/// every entry is stored and reported, one more NEW use case is refused with the service's
/// own sentence, and reconfiguring a use case the assistant already carries is still
/// served — the list does not grow.
#[tokio::test]
async fn orchestrator_use_cases_are_stored_up_to_the_ceiling_and_refused_past_it() {
    let emulator = Emulator::new();
    let (assistant, _) = fresh_assistant(&emulator, "orchestrator ceiling").await;
    let agent = agent_of_type(&emulator, &assistant, ORCHESTRATION, "orchestrator ceiling").await;

    let configure_use_case = |use_case: String| {
        let assistant = assistant.clone();
        let agent = agent.clone();
        let emulator = &emulator;
        async move {
            configure(
                emulator,
                &assistant,
                json!({
                    "aiAgentType": ORCHESTRATION,
                    "configuration": { "aiAgentId": agent },
                    "orchestratorUseCase": use_case,
                }),
            )
            .await
        }
    };

    for index in 0..MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT {
        let reply = configure_use_case(format!("Connect.UseCase{index}")).await;
        assert_eq!(
            reply.status, 200,
            "case \"use case {index}\": expected HTTP 200 up to the ceiling, got {} with \
             body {}",
            reply.status, reply.body
        );
    }
    let read = get_assistant(&emulator, &assistant).await;
    let stored = orchestrators(&read, "/assistant")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        stored, MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT,
        "case \"at the ceiling\": expected {MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT} \
         stored entries, got {stored}"
    );

    // One more use case than the ceiling admits.
    let refused = configure_use_case("Connect.OnePastTheCeiling".to_string()).await;
    assert_eq!(
        refused.status, 400,
        "case \"one past the ceiling\": expected HTTP 400, got {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type, CODE_VALIDATION,
        "case \"one past the ceiling\": expected error code {CODE_VALIDATION}, got {:?}",
        refused.error_type
    );
    assert_eq!(
        refused.string("/message"),
        message_orchestrator_quota(),
        "case \"one past the ceiling\": expected the service's own sentence, got {:?}",
        refused.string("/message")
    );

    // A use case the assistant already carries is reconfigured, not refused: replacing an
    // entry does not grow the list.
    let replaced = configure_use_case("Connect.UseCase0".to_string()).await;
    assert_eq!(
        replaced.status, 200,
        "case \"reconfigure at the ceiling\": expected HTTP 200, got {} with body {}",
        replaced.status, replaced.body
    );
    let stored = orchestrators(&replaced, "/assistant")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        stored, MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT,
        "case \"reconfigure at the ceiling\": expected the list to stay at \
         {MAX_ORCHESTRATOR_CONFIGURATIONS_PER_ASSISTANT}, got {stored}"
    );
}
