Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the wellarchitected emulator

_Filed: 2026-10-21T21:18:47Z_
_Operation: service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 95 operations in `services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501. The service holds no state, so no client workflow works.

This issue scopes the FIRST buildable slice: the workload lifecycle, which is the primary resource of the Well-Architected Tool (`restJson1`, sdkId `WellArchitected`). Implement these 5 operations statefully, in this order:

1. **CreateWorkload** — `POST /workloads`. Required input: `WorkloadName` (3..100), `Description` (3..250), `Environment` (PREPRODUCTION|PRODUCTION), `Lenses` (list<string>), `ClientRequestToken` (@idempotencyToken). Also accept and PERSIST `AccountIds`, `AwsRegions`, `NonAwsRegions`, `PillarPriorities`, `ArchitecturalDesign`, `ReviewOwner`, `IndustryType`, `Industry`, `Notes`, `Tags`, `Applications`. Generate a new `WorkloadId` matching `^[0-9a-f]{32}$` and a `WorkloadArn` of the form `arn:aws:wellarchitected:<region>:<account>:workload/<WorkloadId>`; return both. Store the whole record. Must reject a duplicate `WorkloadName` (name is unique per account+region, case/space-insensitive) with `ConflictException` (409), and honour `ClientRequestToken` idempotency: a replay with the same token and parameters returns the ORIGINAL `WorkloadId` instead of creating a second workload.
2. **GetWorkload** — `GET /workloads/{WorkloadId}`. Returns `Workload` rebuilt from the stored record: `WorkloadId`, `WorkloadArn`, `WorkloadName`, `Description`, `Environment`, `UpdatedAt`, `AccountIds`, `AwsRegions`, `NonAwsRegions`, `ArchitecturalDesign`, `ReviewOwner`, `IndustryType`, `Industry`, `Notes`, `ImprovementStatus`, `PillarPriorities`, `Lenses`, `Owner`, `Tags`, `RiskCounts`. Unknown id -> `ResourceNotFoundException` (404), never a canned workload.
3. **ListWorkloads** — `POST /workloadsSummaries`. Returns `WorkloadSummaries` (list<WorkloadSummary>: `WorkloadId`, `WorkloadArn`, `WorkloadName`, `Owner`, `UpdatedAt`, `Lenses`, `RiskCounts`, `ImprovementStatus`) built by projecting the stored workloads. Must filter by `WorkloadNamePrefix`, cap the page at `MaxResults`, and emit/consume an opaque `NextToken` (`^[A-Za-z0-9+/=_-]+$`) so paging over >MaxResults workloads enumerates each exactly once and the last page omits `NextToken`.
4. **UpdateWorkload** — `PATCH /workloads/{WorkloadId}`. Applies only the members present in the request (`WorkloadName`, `Description`, `Environment`, `AccountIds`, `AwsRegions`, `NonAwsRegions`, `PillarPriorities`, `ArchitecturalDesign`, `ReviewOwner`, `IsReviewOwnerUpdateAcknowledged`, `IndustryType`, `Industry`, `Notes`, `ImprovementStatus`, `Applications`), refreshes `UpdatedAt`, persists the merge, and returns the FULL updated `Workload`. Absent members must be left untouched (not nulled). Unknown id -> `ResourceNotFoundException` (404).
5. **DeleteWorkload** — `DELETE /workloads/{WorkloadId}?ClientRequestToken=...`. Removes the record (and its shares/milestones/lens reviews) and returns an empty 200 body. A second delete or a later `GetWorkload` for that id must return `ResourceNotFoundException` (404), and the workload must disappear from `ListWorkloads`.

No hollow responses: each of the five must compute its response from the shared store reached via `crate::state::service_state(&*self.engine)`. Hardcoded ids, echoed inputs, or a generated `WorkloadId` that is never stored all count as failures of this issue. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each of the five handler files as it is implemented.

## State Model

In `src/state.rs` (`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState`):

- `workloads: HashMap<String /* WorkloadId (32 lowercase hex) */, Workload>` where `Workload { workload_id: String, workload_arn: String, workload_name: String, description: String, environment: WorkloadEnvironment, updated_at: SystemTime, account_ids: Vec<String>, aws_regions: Vec<String>, non_aws_regions: Vec<String>, pillar_priorities: Vec<String>, architectural_design: Option<String>, review_owner: Option<String>, is_review_owner_update_acknowledged: bool, industry_type: Option<String>, industry: Option<String>, notes: Option<String>, improvement_status: WorkloadImprovementStatus /* default NOT_APPLICABLE */, lenses: Vec<String>, owner: String /* 12-digit account id */, tags: BTreeMap<String,String>, applications: Vec<String>, risk_counts: BTreeMap<String,i32>, discovery_config: Option<WorkloadDiscoveryConfig> }`
- `workload_name_index: HashMap<String /* normalized (lowercased, spaces removed) name */, String /* WorkloadId */>` — backs the `ConflictException` on duplicate names.
- `create_workload_tokens: HashMap<String /* ClientRequestToken */, String /* WorkloadId */>` — backs `@idempotencyToken` replay.
- `list_tokens: HashMap<String /* NextToken */, usize /* offset into the name-sorted workload id list */>` (or an encoded opaque cursor) — backs `ListWorkloads` paging.

All four live behind the one `state.lock()` guard so a create is visible to the next request on the same engine.

Everything else (lens reviews, answers, milestones, shares, profiles, review templates, agent/recommendation ops, tagging) stays 501 and is tracked by separate per-operation gap issues.

## Evidence

`services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/OPERATIONS.md` (95 operations, protocol `restJson1`, sdkId `WellArchitected`) documents each route and member cited above: CreateWorkload (lines 113-118), DeleteWorkload (193-198), GetWorkload (360-365), ListWorkloads (557-563, `@paginated(inputToken=NextToken, outputToken=NextToken)`), UpdateWorkload (698-703). The `Workload` and `WorkloadSummary` output shapes and their members come from `smithy_query shape=Workload,WorkloadSummary` on the on-disk model. `src/ops/create_workload.rs` lines 4 and 231 show the scaffold marker and `Err(crate::scaffolded::Scaffolded::not_implemented("CreateWorkload"))`; every other file in `src/ops/` is the same generated stub. Documented error semantics (409 ConflictException on duplicate workload name, 404 ResourceNotFoundException for unknown ids, ClientRequestToken idempotency as described in the CreateWorkload doc comment, lines 74-82) are modeled errors for these operations. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI wellarchitected client at it.
2. `aws wellarchitected create-workload --workload-name demo-workload --description 'bootstrap demo' --environment PRODUCTION --lenses wellarchitected --aws-regions us-east-1 --review-owner owner@example.com --client-request-token tok-1` — today returns HTTP 501 NotImplemented; expected 200 with a 32-hex `WorkloadId` and a `WorkloadArn`.
3. Repeat the identical call with `--client-request-token tok-1`: expected the SAME `WorkloadId` back (idempotent replay), not a second workload.
4. Call create-workload again with the same name but a new token: expected `ConflictException` (HTTP 409).
5. `aws wellarchitected get-workload --workload-id <WorkloadId>`: expected the workload exactly as created (name, description, environment PRODUCTION, lenses, aws-regions, review owner, `UpdatedAt` set).
6. `aws wellarchitected list-workloads --workload-name-prefix demo --max-results 1`: expected the new workload in `WorkloadSummaries`; create a second workload and page with `--max-results 1` + `--next-token` to see each workload exactly once and no `NextToken` on the final page.
7. `aws wellarchitected update-workload --workload-id <WorkloadId> --description 'updated desc' --improvement-status IN_PROGRESS`: expected 200 with the full `Workload` showing the new description/status, the original `WorkloadName`/`Lenses` untouched, and a bumped `UpdatedAt`; a follow-up get-workload must show the same.
8. `aws wellarchitected delete-workload --workload-id <WorkloadId> --client-request-token tok-del`: expected empty 200; then get-workload on that id must return `ResourceNotFoundException` (404) and list-workloads must no longer include it.
