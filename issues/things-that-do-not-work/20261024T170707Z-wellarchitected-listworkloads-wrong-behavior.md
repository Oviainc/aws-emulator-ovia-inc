Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6f4880dda1aa7f89588445b4a5ba9a411741294f

# Well-Architected state is not Region-scoped: a workload created in one Region is listed and readable in every other Region

_Filed: 2026-10-24T17:07:07Z_
_Operation: ListWorkloads | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6f4880dda1aa7f89588445b4a5ba9a411741294f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The Well-Architected Tool is a regional service: a workload, custom lens, review template or profile created against the us-east-1 endpoint exists only in us-east-1, its ARN carries that Region, and the documented workload-name uniqueness rule is scoped "within an account within an Amazon Web Services Region".

The emulator keeps ONE global store for every resource, keyed only by id/ARN, and uses the request's signed Region solely to mint ARNs. Observable divergences:

* Expected: `POST /workloadsSummaries` (ListWorkloads) signed for eu-west-1 reports an empty `WorkloadSummaries` list when the only workload was created in us-east-1. Actual: it reports that workload, with the `WorkloadArn` that embeds `us-east-1` — i.e. an eu-west-1 call hands back a us-east-1 ARN.
* Expected: `GetWorkload`/`ListLensReviews`/`ListMilestones` for that id against a different Region answer 404 `ResourceNotFoundException`. Actual: they answer 200 from the cross-Region record.
* Expected: `CreateWorkload` with the same `WorkloadName` in a second Region succeeds (the uniqueness rule is per Region). Actual: 409 `ConflictException`, because `workload_name_index` is one global map.
* The same applies to `ListReviewTemplates`, `ListLenses` (custom lenses imported in another Region are listed, while an official lens's ARN is rewritten to the caller's Region), `GetProfile` and `ListAgentProfiles`.

## Evidence

Spec: `WorkloadName` doc — "The name must be unique within an account within an Amazon Web Services Region" (`smithy_query(query="shape", shape="WorkloadName")`); `WorkloadArn`/`LensArn`/`TemplateArn`/`ProfileArn` all embed a Region (see the ARN patterns in OPERATIONS.md for CreateProfile/CreateReviewTemplate), which is only meaningful for a per-Region resource namespace.

Handler/state: `src/state.rs` declares one un-partitioned store — `pub workloads: HashMap<String, WorkloadRecord>` (line ~3643), `pub review_templates: HashMap<String, ReviewTemplateRecord>` (~3685), `pub profiles: HashMap<String, ProfileRecord>` (~3697), `pub lenses: HashMap<String, LensRecord>` (~3703), `pub workload_name_index: HashMap<String, String>` (~3667) — and `service_state()` (~5611) returns that single store regardless of the request. `request_region(parts)` (~5601) is read only where an ARN is minted (`src/ops/create_workload.rs:480`, `mint_profile_arn`, `mint_review_template_arn`, `ImportLens`). `src/ops/list_workloads.rs:145` discards the context entirely (`let _ = ctx;`) and pages over `store.workload_order()`, which enumerates every workload in the engine.

Tests: `tests/all/workload_lifecycle.rs` and `tests/all/workload_harness.rs` drive one Region only; no test asserts cross-Region visibility either way, so nothing pins the current behaviour.

## Steps to Reproduce

1. CreateWorkload with a SigV4 credential scope for us-east-1 (WorkloadName "alpha", one lens, ReviewOwner set); note the returned WorkloadId and WorkloadArn (contains us-east-1)
2. Call ListWorkloads (POST /workloadsSummaries) with a credential scope for eu-west-1 — the us-east-1 workload is reported, with its us-east-1 ARN, instead of an empty page
3. Call GetWorkload for that WorkloadId signed for eu-west-1 — 200 instead of 404 ResourceNotFoundException
4. CreateWorkload again signed for eu-west-1 with the same WorkloadName "alpha" — 409 ConflictException, although the documented uniqueness rule is per account AND Region
