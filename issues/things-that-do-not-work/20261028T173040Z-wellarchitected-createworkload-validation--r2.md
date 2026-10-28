Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 62eaf53b582c9d7bec5520c65fe502b1fdb205cf

# PillarPriorities entries are stored without ever being resolved to a pillar, although every other place this crate takes a PillarId refuses one no lens defines

_Filed: 2026-10-28T17:30:40Z_
_Operation: CreateWorkload | Class: validation_

> _Note: this issue was identified against the code base at commit `62eaf53b582c9d7bec5520c65fe502b1fdb205cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkload` (and, on the same rule, `UpdateWorkload`) writes the request's `PillarPriorities` list into the stored workload verbatim. No entry is ever checked against the pillars the workload's lenses define, so a workload can be created — and later read back — holding priorities such as `["not-a-pillar", "SECURITY", ""]`: strings that are no `PillarId` of any lens review the workload carries.

This crate otherwise treats a `PillarId` arriving in a request body as a value that has to resolve. `UpdateLensReview` refuses a `PillarNotes` key, and a Jira `SelectedPillars` entry, that names a pillar the lens does not define, through the shared helper `crate::state::undefined_pillar_refusal`; `UpdateReviewTemplateLensReview` applies the identical rule to a template's pillar notes; `CreateAgentGoal`/`UpdateAgentGoal`/`UpdateAgentProfile` refuse a pillar outside the published set through `crate::state::unpublished_pillar_exception`, with the stated reason that a resource must never be left "STORED naming a pillar the enum does not publish". The workload's own pillar order is the one `PillarId` list in the service that is taken on trust — even though the crate already knows the published pillar set (`crate::state::default_pillar_priorities`, built from `WELL_ARCHITECTED_PILLARS`) and uses exactly that set as the value a request which names none is given.

The effect is a stored workload whose `PillarPriorities` cannot be acted on by anything: it is a list of pillars that do not exist, echoed back by `GetWorkload`, `UpdateWorkload` and every milestone that snapshots the workload.

## Evidence

Model — the member states its entries ARE pillar ids: `smithy_query(query="shape", shape="WorkloadPillarPriorities")` → "The priorities of the pillars, which are used to order items in the improvement plan. Each pillar is represented by its PillarReviewSummary$PillarId." with `member: {target: PillarId}`, and `PillarId` is "The ID used to identify a pillar, for example, `security`. A pillar is identified by its PillarReviewSummary$PillarId."

The gap — `src/ops/create_workload.rs:324-331`:
```
// "The priorities of the pillars, which are used to order items in the
// improvement plan" (`PillarPriorities` doc): a workload is always reviewed in
// some pillar order, so a request that names none is created with the
// framework's own pillar order rather than with no priorities at all.
let pillar_priorities = match input.pillar_priorities {
    Some(priorities) if !priorities.is_empty() => priorities,
    _ => crate::state::default_pillar_priorities(),
};
```
stored unchecked at `src/ops/create_workload.rs:545` (`pillar_priorities,`) and reported at `src/state.rs:2600` (`pillar_priorities: Some(self.pillar_priorities.clone()),`). The same write on the update path, `src/ops/update_workload.rs:355-357`:
```
if let Some(pillar_priorities) = input.pillar_priorities {
    record.pillar_priorities = pillar_priorities;
}
```
No caller of a pillar-resolution helper exists on either path: `grep -n 'pillar_priorities' src/` lists only `state.rs:2069/2600/4115`, `create_workload.rs:328/330/351/545` and `update_workload.rs:325/355/356` — no `undefined_pillar_refusal`, no `unpublished_pillar_exception`.

The sibling rule this crate already implements — `src/ops/update_lens_review.rs:218-231`:
```
// A `PillarNotes` key is a `PillarId`, and the pillars of a lens are what its lens
// content names: a key naming a pillar the lens does NOT define belongs to no
// pillar of this lens review and is refused rather than stored.
if let Some(exception) = crate::state::undefined_pillar_refusal(
    crate::state::PILLAR_NOTES_MEMBER, ... ) {
    return Err(UpdateLensReviewErrorModeled::ValidationException(exception).into());
}
```
and `src/ops/update_agent_profile.rs:205-215`: "It is kept so an update can never leave a profile STORED naming a pillar the enum does not publish, and the refusal is built in `crate::state` so every operation that stores a pillar list words it identically."

The published pillar set is already in the crate — `src/state.rs:4111-4120`:
```
/// The pillar priority order a workload is created with when the request names none:
/// the framework's own pillar order ...
pub fn default_pillar_priorities() -> Vec<String> {
    WELL_ARCHITECTED_PILLARS.iter().map(|(pillar_id, _)| (*pillar_id).to_string()).collect()
}
```

Test coverage that must stay green (every row uses real pillar ids, so enforcing pillar identity changes no existing expectation): `tests/all/workload_computed_members.rs:213-274` (`pillar_priorities_default_to_the_framework_pillar_order`, rows `["sustainability"]`, `["costOptimization","security"]`, all six reversed, `[]`, omitted); `tests/all/workload_lifecycle.rs:41` (`["security","reliability","costOptimization"]`), `:557` (`["security"]`); `tests/all/workload_create_token_parameters.rs:176-181` (fingerprint row `["security","reliability"]` vs `["reliability","security"]`).

## Steps to Reproduce

1. POST /workloads with a valid body and `"PillarPriorities": ["not-a-pillar", "SECURITY", ""]` (every entry satisfies PillarId's modelled @length(1,64) except the empty one, which can be dropped from the row)
2. Observe 200 and a stored workload: GET /workloads/{WorkloadId} reports `PillarPriorities: ["not-a-pillar", "SECURITY"]` — pillar ids no lens review of the workload carries in any PillarReviewSummary$PillarId
3. Compare with PATCH /workloads/{WorkloadId}/lensReviews/wellarchitected carrying `"PillarNotes": {"not-a-pillar": "x"}`, which is refused 400 ValidationException by src/ops/update_lens_review.rs:221-231 for naming a pillar the lens does not define
4. Same gap on the update path: PATCH /workloads/{WorkloadId} with `"PillarPriorities": ["not-a-pillar"]` is accepted and stored (src/ops/update_workload.rs:355-357)
