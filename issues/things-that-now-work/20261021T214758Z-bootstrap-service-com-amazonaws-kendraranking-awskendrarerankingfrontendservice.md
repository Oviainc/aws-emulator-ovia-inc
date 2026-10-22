Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the Kendra Ranking (AWSKendraRerankingFrontendService) emulator

_Filed: 2026-10-21T21:47:58Z_
_Operation: service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice | Class: missing_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 9 operations in `src/ops/` are unmodified scaffolds that answer HTTP 501 `NotImplemented` and hold no state, so no client workflow works at all. Implement the following FIVE operations as the first stateful slice — the create -> read -> list -> use -> delete lifecycle of the service's primary resource, the rescore execution plan. Every response must be computed from persisted state held in the shared engine store (`crate::state::service_state(&*self.engine)`); no canned bodies, no echoing the request back, no generating an Id that is never stored.

1. **CreateRescoreExecutionPlan** (`POST /rescore-execution-plans`) — input `Name` (required, 1..1000, `^[a-zA-Z0-9][a-zA-Z0-9_-]*$`), `Description`, `CapacityUnits` (`CapacityUnitsConfiguration{RescoreCapacityUnits: integer >= 0, required}`), `Tags` (list<Tag>, 0..200), `ClientToken` (@idempotencyToken). Must mint a NEW `Id` that is exactly 36 chars matching `^[a-zA-Z0-9][a-zA-Z0-9-]*$` (a UUID v4 works), build `Arn` as `arn:aws:kendra-ranking:<region>:<account>:rescore-execution-plan/<Id>`, INSERT the plan into the store with `Status = ACTIVE` (the emulator need not simulate a CREATING transition), `CreatedAt = UpdatedAt = now`, and return `{Id, Arn}` (both @required). Repeating the same `ClientToken` must return the SAME `Id`/`Arn` rather than creating a second plan.
2. **DescribeRescoreExecutionPlan** (`GET /rescore-execution-plans/{Id}`) — must look the `Id` up in the store and return the values actually persisted by step 1: `Id`, `Arn`, `Name`, `Description`, `CapacityUnits`, `CreatedAt`, `UpdatedAt`, `Status` (ACTIVE), `ErrorMessage` absent. An unknown `Id` must return `ResourceNotFoundException` (HTTP 404), not a hollow 200 and not 501.
3. **ListRescoreExecutionPlans** (`GET /rescore-execution-plans`) — must return one `RescoreExecutionPlanSummary{Name, Id, CreatedAt, UpdatedAt, Status}` per plan currently in the store, honouring `MaxResults` (1..50) and round-tripping `NextToken` (1..800) so a paged walk visits every plan exactly once; omit `NextToken` on the final page. A deleted plan must not appear.
4. **Rescore** (`POST /rescore-execution-plans/{RescoreExecutionPlanId}/rescore`) — input `SearchQuery` (required, 1..1000) and `Documents` (required, len >= 1, each `Document{Id required, OriginalScore required float -100000..100000, GroupId?, Title?, Body?, TokenizedTitle?, TokenizedBody?}`). Must verify `RescoreExecutionPlanId` exists in the store (else `ResourceNotFoundException` 404), then return a `RescoreId` and a `ResultItems` list of `RescoreResultItem{DocumentId, Score}` containing exactly one entry per input document, with every `DocumentId` drawn from the request and the list ordered by descending `Score`. A deterministic relevance heuristic (e.g. query-term overlap with Title/Body blended with `OriginalScore`) is acceptable — what must hold is that the output is derived from the submitted documents and the plan exists, never a fixed list.
5. **DeleteRescoreExecutionPlan** (`DELETE /rescore-execution-plans/{Id}`) — must REMOVE the plan from the store and return an empty 200. A second delete, and any subsequent Describe or Rescore against that `Id`, must return `ResourceNotFoundException` (HTTP 404), and the plan must disappear from ListRescoreExecutionPlans.

Validation on this slice: reject a `Name` violating its length/pattern, a `Documents` list that is empty, a missing required member, or an `Id` that is not 36 chars with `ValidationException` (HTTP 400) — the modeled 400, not a 501 or a 500.

## State Model

A single service state struct in `src/state.rs`, reached via `crate::state::service_state(&*self.engine)` and mutated under its lock:

- `plans: BTreeMap<String /* plan Id, 36-char uuid */, RescoreExecutionPlan>` — BTreeMap so ListRescoreExecutionPlans has a stable total order for token-based paging.
- `RescoreExecutionPlan { id: String, arn: String, name: String, description: Option<String>, capacity_units: Option<CapacityUnitsConfiguration /* rescore_capacity_units: i32 */>, created_at: SystemTime, updated_at: SystemTime, status: RescoreExecutionPlanStatus /* ACTIVE */, error_message: Option<String>, tags: Vec<Tag /* key, value */> }` — `tags` is stored now (cheap) even though TagResource/UntagResource/ListTagsForResource stay stubbed in this slice.
- `arn_index: HashMap<String /* arn */, String /* plan Id */>` — so the later tagging operations (and any ARN-addressed call) can resolve `ResourceARN` to a plan.
- `client_tokens: HashMap<String /* ClientToken */, String /* plan Id */>` — backs CreateRescoreExecutionPlan idempotency.
- `list_tokens: HashMap<String /* NextToken */, String /* last plan Id returned */>` (or encode the last-seen Id directly in the opaque token) — backs ListRescoreExecutionPlans paging.
- `rescores: HashMap<String /* RescoreId */, Vec<RescoreResultItem { document_id: String, score: f32 }>>` — optional but keeps Rescore results inspectable and proves the result list was computed, not canned.

Out of scope for this first slice and expected to keep answering 501 (to be filed as individual gap issues later): `UpdateRescoreExecutionPlan`, `TagResource`, `UntagResource`, `ListTagsForResource`.

Done = an integration test that creates a plan, describes it and sees the submitted Name/Description/CapacityUnits back, lists it, rescores a 3-document batch and gets 3 scored result items for those document ids, deletes the plan, and then gets 404 from Describe and an empty list from List.

## Evidence

`services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/OPERATIONS.md` lists all 9 operations with the exact input/output members and errors quoted above (CreateRescoreExecutionPlan lines 27-32, DeleteRescoreExecutionPlan 34-39, DescribeRescoreExecutionPlan 41-46, ListRescoreExecutionPlans 48-54, Rescore 62-67), and the README documents the wire protocol as `awsJson1_0` with `X-Amz-Target: AWSKendraRerankingFrontendService.<Op>` plus REST bindings. `smithy_query` on `Document`, `RescoreResultItem`, `CapacityUnitsConfiguration` and `RescoreExecutionPlanSummary` supplies the nested member names/constraints cited in the State Model (`Document.Id` and `Document.OriginalScore` are @required; `RescoreResultItem{DocumentId, Score}`; `CapacityUnitsConfiguration.RescoreCapacityUnits` @required range >= 0). `grep -r 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 9 handler files (create_rescore_execution_plan.rs, describe_rescore_execution_plan.rs, list_rescore_execution_plans.rs, rescore.rs, delete_rescore_execution_plan.rs, update_rescore_execution_plan.rs, tag_resource.rs, untag_resource.rs, list_tags_for_resource.rs), confirming every operation still returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501 and persists nothing. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `kendra-ranking` client at it.
2. `CreateRescoreExecutionPlan {Name: "my-plan", Description: "demo", CapacityUnits: {RescoreCapacityUnits: 2}, ClientToken: "tok-1"}` — expect 200 with a 36-char `Id` and a `kendra-ranking` ARN containing that Id; actual: HTTP 501 NotImplemented.
3. Repeat the identical call with `ClientToken: "tok-1"` — expect the SAME Id/Arn (idempotent), not a second plan.
4. `DescribeRescoreExecutionPlan {Id: <Id>}` — expect 200 echoing the PERSISTED Name="my-plan", Description="demo", CapacityUnits.RescoreCapacityUnits=2, Status=ACTIVE, CreatedAt/UpdatedAt set.
5. `ListRescoreExecutionPlans {MaxResults: 1}` — expect `SummaryItems` containing a summary for <Id> with Status=ACTIVE; create a second plan and page with the returned `NextToken` to see both plans exactly once.
6. `Rescore {RescoreExecutionPlanId: <Id>, SearchQuery: "intelligent ranking", Documents: [{Id: "d1", OriginalScore: 0.4, Title: "intelligent ranking guide", Body: "..."}, {Id: "d2", OriginalScore: 0.9, Body: "unrelated"}, {Id: "d3", OriginalScore: 0.1, Title: "ranking"}]}` — expect 200 with a `RescoreId` and 3 `ResultItems` whose `DocumentId`s are exactly d1/d2/d3, each with a `Score`, sorted descending by Score.
7. `Rescore` against a random unknown 36-char plan id — expect `ResourceNotFoundException` (HTTP 404).
8. `DeleteRescoreExecutionPlan {Id: <Id>}` — expect empty 200; then `DescribeRescoreExecutionPlan {Id: <Id>}` must return `ResourceNotFoundException` (404) and `ListRescoreExecutionPlans` must no longer list it.
9. `CreateRescoreExecutionPlan {Name: "bad name!"}` and `Rescore {..., Documents: []}` — expect `ValidationException` (HTTP 400).
