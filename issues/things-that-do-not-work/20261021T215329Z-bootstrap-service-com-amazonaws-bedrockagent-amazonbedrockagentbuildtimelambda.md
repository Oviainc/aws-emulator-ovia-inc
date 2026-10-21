Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the bedrock-agent emulator

_Filed: 2026-10-21T21:53:29Z_
_Operation: service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda | Class: missing_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 86 operations in `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST stateful slice covering the agent lifecycle — the service's primary resource — so a realistic client script works end-to-end. Scope is exactly these 5 operations (restJson1, sigv4 signing name `bedrock`, endpoint prefix `bedrock-agent`):

1. **CreateAgent** — `PUT /agents/ -> 202`. Input: `agentName` (required, pattern `^([0-9a-zA-Z][_-]?){1,100}$`), `instruction`, `foundationModel`, `description`, `idleSessionTTLInSeconds` (60..5400), `agentResourceRoleArn`, `customerEncryptionKeyArn`, `tags`, `clientToken` (@idempotencyToken), `agentCollaboration`, `orchestrationType`. Output: `agent` (Agent, required). Must mint a NEW `agentId` matching `^[0-9a-zA-Z]{10}$`, build `agentArn` as `arn:aws:bedrock:<region>:<account>:agent/<agentId>`, set `agentVersion` = `DRAFT`, `agentStatus` = `NOT_PREPARED`, `createdAt`/`updatedAt` = now, default `idleSessionTTLInSeconds` to 3600 when absent, and STORE the record. Agent's required members (`agentId`, `agentName`, `agentArn`, `agentVersion`, `agentStatus`, `idleSessionTTLInSeconds`, `agentResourceRoleArn`, `createdAt`, `updatedAt`) must all be populated from the stored record.
2. **GetAgent** — `GET /agents/{agentId}/ -> 200`, @readonly. Input: `agentId`. Output: `agent` (required) — must be the record CreateAgent persisted, including every field the create call supplied (`instruction`, `foundationModel`, `description`, `idleSessionTTLInSeconds`, role/KMS ARNs) and the current `agentStatus`. Unknown `agentId` -> `ResourceNotFoundException` (HTTP 404). This read must not mutate state.
3. **ListAgents** — `POST /agents/ -> 200`, @paginated(inputToken=nextToken, outputToken=nextToken, items=agentSummaries). Input: `maxResults` (1..1000), `nextToken`. Output: `agentSummaries` (list<AgentSummary>, required), `nextToken`. Each summary is projected from the stored agent: `agentId`, `agentName`, `agentStatus`, `updatedAt` (all required), plus `description` and `latestAgentVersion`. Must list exactly the agents that exist, honour `maxResults` by truncating, and return a `nextToken` only when more remain — a token that resumes where the previous page stopped.
4. **PrepareAgent** — `POST /agents/{agentId}/ -> 202`. Input: `agentId`. Output: `agentId`, `agentStatus`, `agentVersion`, `preparedAt` (all required). Must MUTATE the stored agent: set `agentStatus` to `PREPARED` (`PREPARING` is also acceptable if a later GetAgent converges to `PREPARED`), set `preparedAt`/`updatedAt` = now, return `agentVersion` = `DRAFT`. A following GetAgent must observe the new status and `preparedAt`. Unknown `agentId` -> 404 `ResourceNotFoundException`.
5. **DeleteAgent** — `DELETE /agents/{agentId}/ -> 202`. Input: `agentId` (@httpLabel), `skipResourceInUseCheck` (@httpQuery, default false). Output: `agentId`, `agentStatus` (required) — return `DELETING`. Must REMOVE the agent from the store so a subsequent GetAgent answers 404 `ResourceNotFoundException` and ListAgents no longer includes it. Unknown `agentId` -> 404.

No canned, hardcoded, or echoed responses: every one of the five must compute its response from the persisted store reached through `crate::state::service_state(&*self.engine)` (never a freshly constructed engine). An id returned without a stored record, or a GetAgent that echoes nothing/the request, is a failure of this issue even if it returns 2xx. Validation that the framework already enforces from the model (patterns, lengths, ranges) does not need re-checking in the handler; name collisions on CreateAgent should answer `ConflictException` (409).

## State Model

In `src/state.rs`, extend the service state struct with:

- `agents: HashMap<String /* agentId, 10 alnum chars */, Agent>` — the single source of truth for all five operations.
- `Agent { agent_id: String, agent_name: String, agent_arn: String, agent_version: String /* "DRAFT" */, agent_status: AgentStatus /* NOT_PREPARED | PREPARING | PREPARED | DELETING */, instruction: Option<String>, foundation_model: Option<String>, description: Option<String>, idle_session_ttl_in_seconds: i32, agent_resource_role_arn: String, customer_encryption_key_arn: Option<String>, orchestration_type: Option<String>, agent_collaboration: Option<String>, prompt_override_configuration: Option<PromptOverrideConfiguration>, guardrail_configuration: Option<GuardrailConfiguration>, memory_configuration: Option<MemoryConfiguration>, tags: HashMap<String,String>, client_token: Option<String>, created_at: SystemTime, updated_at: SystemTime, prepared_at: Option<SystemTime> }`
- `agent_names: HashMap<String /* agentName */, String /* agentId */>` — so CreateAgent can detect a duplicate name and answer 409.
- `client_tokens: HashMap<String /* clientToken */, String /* agentId */>` — so a repeated CreateAgent with the same `@idempotencyToken` returns the same agent instead of creating a second one.

ListAgents pagination: order agents by `created_at` (ties broken by `agent_id`) and encode `nextToken` as the last-returned `agent_id` (or an offset), decoded on the next call to resume.

The other 81 operations (action groups, aliases, knowledge bases, data sources, flows, prompts, ingestion jobs, tagging, resource policies, VPC configurations, agent versions) stay at 501 and are covered by separate gap issues.

## Evidence

OPERATIONS.md for this crate (`services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/OPERATIONS.md`) documents the wire protocol (restJson1, SDK id `Bedrock Agent`, signing name `bedrock`, endpoint prefix `bedrock-agent`) and all 86 operations; lines 170-215 give the exact routes, input members and required output members for CreateAgent (`PUT /agents/ -> 202`, output `agent` required), DeleteAgent (`DELETE /agents/{agentId}/ -> 202`, output `agentId`+`agentStatus` required, `skipResourceInUseCheck` @httpQuery default false), GetAgent (`GET /agents/{agentId}/ -> 200`, @readonly, 404 ResourceNotFoundException modeled), ListAgents (`POST /agents/ -> 200`, @paginated inputToken/outputToken=nextToken items=agentSummaries, maxResults 1..1000) and PrepareAgent (`POST /agents/{agentId}/ -> 202`, output `agentId`/`agentStatus`/`agentVersion`/`preparedAt` all required). smithy_query on shapes `Agent` and `AgentSummary` confirms the required members cited in the State Model (Agent.agentId/agentName/agentArn/agentVersion[pattern ^DRAFT$]/agentStatus/idleSessionTTLInSeconds/agentResourceRoleArn/createdAt/updatedAt; AgentSummary.agentId/agentName/agentStatus/updatedAt) and the `agentArn` pattern `^arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:agent/[0-9a-zA-Z]{10}$`. Handler evidence: `src/ops/create_agent.rs` lines 3-4 carry `//! @op CreateAgent` + `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 229 is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateAgent"))`; all 86 files under `src/ops/` are the same generated scaffold, so no test asserts any agent behaviour today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `bedrock-agent` client at it.
2. `CreateAgent` with agentName="demo-agent", foundationModel="anthropic.claude-v2", instruction=<40+ chars>, agentResourceRoleArn="arn:aws:iam::123456789012:role/AgentRole" — expect 202 with `agent.agentId` matching ^[0-9a-zA-Z]{10}$, agentStatus=NOT_PREPARED, agentVersion=DRAFT, a well-formed agentArn, and createdAt/updatedAt set. Today: HTTP 501 NotImplemented.
3. `GetAgent` with the returned agentId — expect 200 and the SAME agent: agentName="demo-agent", the instruction and foundationModel supplied at create time, idleSessionTTLInSeconds populated.
4. `ListAgents` with maxResults=10 — expect the new agent present in `agentSummaries` with agentId/agentName/agentStatus/updatedAt; create a second agent and call again with maxResults=1 to confirm truncation plus a `nextToken` that returns the remaining agent on the follow-up call.
5. `PrepareAgent` with the agentId — expect 202 with agentStatus (PREPARING/PREPARED), agentVersion=DRAFT and preparedAt set; then `GetAgent` again and confirm the stored agent now reports PREPARED with the preparedAt timestamp.
6. `DeleteAgent` with the agentId — expect 202 with agentStatus=DELETING; then `GetAgent` on that id must return HTTP 404 ResourceNotFoundException and `ListAgents` must no longer include it.
7. `GetAgent` with a never-created id like "AAAAAAAAAA" — expect HTTP 404 ResourceNotFoundException, not 501 and not a fabricated agent.
