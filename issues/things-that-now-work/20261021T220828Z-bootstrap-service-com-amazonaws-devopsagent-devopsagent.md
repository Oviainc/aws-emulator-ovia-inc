Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the DevOps Agent (aidevops) emulator

_Filed: 2026-10-21T22:08:28Z_
_Operation: service-com-amazonaws-devopsagent-devopsagent | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 63 operations in `services/service-com-amazonaws-devopsagent-devopsagent/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented` while holding zero state. Nothing can be exercised end-to-end today.

This issue defines the FIRST, SMALL slice: the AgentSpace lifecycle plus one backlog-task workflow inside it. The service is `restJson1`, sigv4 signing name `aidevops`.

Implement these 6 operations, in this order, each computing its response from persisted state:

1. **CreateAgentSpace** — `POST /v1/agentspaces -> 201`. Input: `name` (required, 1..255), `description`, `locale`, `kmsKeyArn`, `clientToken` (@idempotencyToken), `tags` (map), `preferences` (map<AgentSpacePreferenceKey(`elevatedActionsEnabled`),boolean>, max 25). Output: `agentSpace` (AgentSpace: `agentSpaceId` matching `^[a-zA-Z0-9-]{1,64}$`, `name`, `description`, `locale`, `kmsKeyArn`, `preferences`, `createdAt`, `updatedAt`) + `tags`. Must mint and STORE a new `agentSpaceId`; the operation is `@idempotent`, so a repeat with the same `clientToken` must return the same agent space rather than creating a second one.
2. **GetAgentSpace** — `GET /v1/agentspaces/{agentSpaceId} -> 200`. Input: `agentSpaceId` (@httpLabel), `includedData` (`ALL_DATA|METADATA_ONLY`). Output: the stored `agentSpace` + `tags`, byte-for-byte what CreateAgentSpace persisted (same id, name, description, createdAt). Unknown id -> `ResourceNotFoundException` (HTTP 404), not 501 and not an empty 200.
3. **ListAgentSpaces** — `POST /v1/agentspaces/list -> 200`. Input: `maxResults` (default 20), `nextToken`, `includedData`. Output: `agentSpaces` (required list<AgentSpace>) + `nextToken`. Must enumerate the stored map, honour `maxResults` as a page bound and round-trip an opaque `nextToken` (@paginated inputToken=nextToken, outputToken=nextToken, items=agentSpaces). A newly created space must appear; a deleted one must not.
4. **CreateBacklogTask** — `POST /backlog/agent-space/{agentSpaceId}/tasks -> 201`. Input: `agentSpaceId` (@httpLabel), `taskType` (`EVALUATION|INVESTIGATION|RELEASE_READINESS_REVIEW|RELEASE_TESTING`, required), `title` (required, 1..400), `description`, `priority` (`CRITICAL|HIGH|MEDIUM|LOW|MINIMAL`, required), `reference`, `clientToken`. Output: `task` (Task: `agentSpaceId`, `taskId`, `title`, `description`, `taskType`, `priority`, `status`, `createdAt`, `updatedAt`, `version`). Must mint a `taskId` matching `^[a-zA-Z0-9_.-]+$`, store it under the parent agent space, set an initial `status` of `PENDING_TRIAGE` (or `PENDING_START`) and `version` = 1. An unknown `agentSpaceId` -> `ResourceNotFoundException` (404).
5. **GetBacklogTask** — `GET /backlog/agent-space/{agentSpaceId}/tasks/{taskId} -> 200`. Output: `task`, read back from state with the exact `title`, `priority`, `taskType` and `createdAt` that CreateBacklogTask stored. Unknown `taskId` (or task belonging to a different agent space) -> `ResourceNotFoundException` (404).
6. **DeleteAgentSpace** — `DELETE /v1/agentspaces/{agentSpaceId} -> 204`, empty body. Must remove the agent space (and its tasks) from state so a subsequent GetAgentSpace returns 404 and ListAgentSpaces omits it. `@idempotent`: deleting an already-deleted id must not 500.

No canned, echoed or hardcoded responses, and no minted id without a corresponding store write: every field in every response above must be read back out of the shared state in `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`), so a create followed by a read returns what was created.

## State Model

In `ComAmazonawsDevopsagentDevopsagentState`:

- `agent_spaces: HashMap<String /* agentSpaceId */, AgentSpaceRecord>` where
  `AgentSpaceRecord { agent_space_id: String, name: String, description: Option<String>, locale: Option<String>, kms_key_arn: Option<String>, preferences: HashMap<String /* AgentSpacePreferenceKey */, bool>, tags: HashMap<String, String>, created_at: SystemTime, updated_at: SystemTime, tasks: HashMap<String /* taskId */, TaskRecord> }`
- `TaskRecord { task_id: String, agent_space_id: String, title: String, description: Option<String>, task_type: TaskType, priority: Priority, status: TaskStatus, reference: Option<ReferenceOutput>, execution_id: Option<String>, metadata: Option<Document>, created_at: SystemTime, updated_at: SystemTime, version: i32 }`
- `client_tokens: HashMap<String /* clientToken */, String /* created resource id */>` so the `@idempotent` creates (CreateAgentSpace, CreateBacklogTask) replay the first result instead of duplicating.
- `next_id: u64` (or a UUID source) used to mint `agentSpaceId` / `taskId` values that satisfy their modeled patterns.

Done = a client script can run CreateAgentSpace -> GetAgentSpace -> ListAgentSpaces -> CreateBacklogTask -> GetBacklogTask -> DeleteAgentSpace with correct, stateful bodies at every step (not merely 2xx), and the delete makes the space observably gone.

The other 57 operations (assets, triggers, chats/messages, associations, private connections, service registry, tagging, recommendations, goals, journal) stay 501 for now and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-devopsagent-devopsagent/OPERATIONS.md` lists 63 operations from `smithy-model/com-amazonaws-devopsagent-devopsagent.json`, with the routes, required members and errors quoted above (CreateAgentSpace `POST /v1/agentspaces -> 201`; GetAgentSpace `GET /v1/agentspaces/{agentSpaceId}`; ListAgentSpaces `POST /v1/agentspaces/list` with @paginated items=agentSpaces; CreateBacklogTask `POST /backlog/agent-space/{agentSpaceId}/tasks -> 201`; GetBacklogTask `GET /backlog/agent-space/{agentSpaceId}/tasks/{taskId}`; DeleteAgentSpace `DELETE /v1/agentspaces/{agentSpaceId} -> 204`). smithy_query on shapes `AgentSpace`, `Task`, `TaskStatus` and `AgentSpacePreferenceKey` supplies the required response members (`agentSpaceId`, `createdAt`, `updatedAt`, `version`, `status`) and the TaskStatus enum values. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 63 handler files, including `create_agent_space.rs`, `get_agent_space.rs`, `list_agent_spaces.rs`, `create_backlog_task.rs`, `get_backlog_task.rs` and `delete_agent_space.rs` — no behavior exists and no tests cover these paths.

## Steps to Reproduce

1. Start the emulator and point an `aidevops` (DevOps Agent) client at it.
2. POST /v1/agentspaces with {"name":"demo-space","description":"bootstrap demo","clientToken":"tok-1"} — expect 201 with agentSpace.agentSpaceId, name="demo-space", createdAt/updatedAt; actual: HTTP 501 NotImplemented.
3. GET /v1/agentspaces/{agentSpaceId} — expect 200 returning exactly the space just created; actual: 501.
4. POST /v1/agentspaces/list?maxResults=20 — expect 200 with the new space present in agentSpaces; actual: 501.
5. POST /backlog/agent-space/{agentSpaceId}/tasks with {"taskType":"INVESTIGATION","title":"investigate latency","priority":"HIGH","clientToken":"tok-2"} — expect 201 with task.taskId, status PENDING_TRIAGE, version 1; actual: 501.
6. GET /backlog/agent-space/{agentSpaceId}/tasks/{taskId} — expect 200 with title="investigate latency", priority=HIGH, taskType=INVESTIGATION; actual: 501.
7. GET /backlog/agent-space/{agentSpaceId}/tasks/does-not-exist — expect 404 ResourceNotFoundException; actual: 501.
8. DELETE /v1/agentspaces/{agentSpaceId} — expect 204 with empty body, then GET /v1/agentspaces/{agentSpaceId} returns 404 and the space is absent from ListAgentSpaces; actual: 501 for all three.
