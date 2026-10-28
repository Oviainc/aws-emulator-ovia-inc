Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 5b375a973f59b1ba0f091f974f00cc96321aba54

# An UpdateAgentContext that carries no writable member still stamps lastModifiedBy/lastModifiedAt, breaking the no-op-update rule this crate states and implements elsewhere

_Filed: 2026-10-28T20:54:45Z_
_Operation: UpdateAgentContext | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `5b375a973f59b1ba0f091f974f00cc96321aba54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateAgentContext`'s only writable members are `title` and `content`, and both are optional (`OPERATIONS.md:607`), so `PUT /api/v1/agent-profiles/{profileArn}/contexts/{id}` with neither of them is a well-formed request that asks for no change. The handler applies the two `if let Some(...)` writes (neither of which fires) and then stamps the modification members unconditionally, so the stored context is left reporting `lastModifiedBy` = the emulator account and `lastModifiedAt` = the instant of a call that wrote nothing. Every later `GetAgentContext` / `ListAgentContexts` then reports the context as having been modified by an update that changed none of its members.

This crate states the opposite rule for exactly this shape of request and implements it in its other update handlers: `UpdateWorkload` computes a `writes_a_member` predicate and only then moves `UpdatedAt`; `UpdateProfile` moves neither the version nor `UpdatedAt` unless something actually changed; `PutAgentRecommendationFeedback` — the sibling write in the same agent plane — guards on `already_held` so a resubmission leaves the stored instant alone. `UpdateAgentContext` has no such guard.

The same unguarded stamp exists in the two sibling agent updates (`update_agent_goal.rs:257-260`, `update_agent_profile.rs:303-308`); it is reported here on `UpdateAgentContext` as the one whose handler doc also claims the change is what is stamped. Adding the guard changes no response for a request that does write a member, so every existing test row stays green: the `clientToken` suite's untokened repeat supplies `title` and `content` (`tests/all/agent_context_updates.rs:566-587`), so the instant it pins still moves.

## Evidence

Side 1 — the unconditional stamp, on a path where both writable members may be absent: `src/ops/update_agent_context.rs:243-262` — "\"Updates an existing context associated with a profile\": every updatable member is optional, so the request carries the members it changes and the STORED value stands for every member it leaves out." followed by `if let Some(title) = input.title { record.title = title; }` / `if let Some(content) = input.content { record.content = content; }` and then, with no guard at all, `record.last_modified_by = Some(::engine::emulator_account_id().to_string());` / `record.last_modified_at = Some(now);`. The model allows the request: `OPERATIONS.md:607` — "**Input:** `clientToken` (string, @idempotencyToken), `profileArn` (… required), `id` (… required), `title` (string), `content` (ContextContent)" — only `profileArn` and `id` are required.

Side 2 — the rule this crate states for an update whose every writable member is optional: `tests/all/no_op_updates.rs:5-9` — "The rule these pin, over tables of inputs rather than one recorded request: an update whose every WRITABLE member is optional treats a request that supplies NONE of them as asking for no change — it brings no record into existence and moves no `UpdatedAt`, and reports the resource exactly as it stands — while a member the request DOES supply is a write even when the value it carries is empty."

Side 2b — the sibling handlers that implement it. `src/ops/update_workload.rs:316-318` — "Every member a request can write here is optional, so a request that supplies NONE of them asks for no change: it reports the workload as it stands and leaves its `UpdatedAt` where it was, rather than recording an update nothing wrote." with `let writes_a_member = input.workload_name.is_some() || …` (`:319-335`) and `if writes_a_member { record.updated_at = std::time::SystemTime::now(); }` (`:390-392`). `src/ops/update_profile.rs:211-223` — "A request that records nothing new — one that supplies no writable member, names no question, or carries exactly what the profile already holds — revises nothing, so it moves neither the version nor `Profile$UpdatedAt` …" with `if description != profile.profile_description || questions != profile.questions { … profile.updated_at = ::std::time::SystemTime::now(); }`. `src/ops/put_agent_recommendation_feedback.rs:223-243` — "`@idempotent`: a resubmission of the feedback this recommendation already holds leaves it exactly as it stood, the instant it was accepted at included" with `let already_held = …; if !already_held { record.feedback = Some(…) }`.

Test coverage that must stay unchanged by the fix: `tests/all/agent_context_updates.rs:566-587` ("The SAME body WITHOUT the token is not a replay, so it is applied: the modification instant moves") — that row sends `{"title": …, "content": …}`, i.e. it supplies writable members, so a `writes_a_member` guard leaves it passing; `tests/all/agent_context_updates.rs:260-273` (a successful update records `lastModifiedBy`/`lastModifiedAt`) likewise supplies members.

## Steps to Reproduce

1. CreateAgentProfile, then CreateAgentContext to obtain a context id: the created context reports lastModifiedBy and lastModifiedAt absent (src/ops/create_agent_context.rs:267-270 — "Nothing has modified a context that has just been created, so the `lastModified…` members are absent rather than echoing the create.")
2. Send PUT /api/v1/agent-profiles/<profileArn>/contexts/<id> with the body {} — no clientToken, no title, no content (both are optional per OPERATIONS.md:607, so the boundary accepts it)
3. Observe 200 and a `context` whose lastModifiedBy is the emulator account and whose lastModifiedAt is the instant of this call, although no member of the context changed (src/ops/update_agent_context.rs:260-261)
4. GET the same context: it keeps reporting that modification, so the store now records an update that wrote nothing
5. Compare with the same shape of request on UpdateWorkload (PATCH /workloads/{WorkloadId} with an empty body), which leaves Workload$UpdatedAt where it was via the writes_a_member guard at src/ops/update_workload.rs:319-335 and :390-392
