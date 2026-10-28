Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba27fe699c954bc2ce7c4c631fb862b19a4228d3

# The improvement plan is ordered by the lens's question order, so the workload's stored PillarPriorities orders nothing

_Filed: 2026-10-28T16:34:09Z_
_Operation: ListLensReviewImprovements | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba27fe699c954bc2ce7c4c631fb862b19a4228d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Workload$PillarPriorities` is modelled as "The priorities of the pillars, which are used to order items in the improvement plan", and this crate restates that sentence twice as the rule it implements. But `ListLensReviewImprovements` builds the plan as a filtered walk of the LENS's own question list and never reads `WorkloadRecord::pillar_priorities` at all: the member is stored by `CreateWorkload`/`UpdateWorkload` and echoed back by `to_workload`, and that is the whole of its effect.

Consequence: a workload whose priorities are, say, `["sustainability", "security"]` still has its improvement items served operationalExcellence-first, in the framework's catalog order — the same order every other workload gets. The one member the model says exists in order to order this list has no influence on it, so two workloads that differ only in `PillarPriorities` serve byte-identical improvement pages.

A grep of the crate shows `pillar_priorities` is reachable from exactly three places (the record field, the `to_workload` projection, and the two workload writers) and from nowhere in `list_lens_review_improvements.rs`, so nothing consumes the ordering the member is for.

## Evidence

Model sentence that states the behaviour — `smithy_query(query="doc", shape="WorkloadPillarPriorities")`: "The priorities of the pillars, which are used to order items in the improvement plan. Each pillar is represented by its PillarReviewSummary$PillarId."

The crate restating that same rule as its own: `src/state.rs:4111-4114` — "/// The pillar priority order a workload is created with when the request names none: /// the framework's own pillar order, which is what orders the items in a review's /// improvement plan (`WorkloadPillarPriorities` doc: \"The priorities of the pillars, /// which are used to order items in the improvement plan\")." and `src/ops/create_workload.rs:324-327` — "// \"The priorities of the pillars, which are used to order items in the // improvement plan\" (`PillarPriorities` doc): a workload is always reviewed in // some pillar order, so a request that names none is created with the // framework's own pillar order rather than with no priorities at all."

The handler that builds the plan, ordering by the lens instead: `src/ops/list_lens_review_improvements.rs:235-237` — "// The improvement plan is a SELECTION over the lens's questions, in the lens's own // order: the questions whose stored answer leaves a risk, narrowed further by the // request's filters and paged through the service's shared question cursor.", implemented at `:251-269` as `let questions = store.lens_review_questions(&input.lens_alias, workload.lens_review_version(&input.lens_alias)); let page = crate::state::question_page(&questions, |question| { priority_selects && input.pillar_id... }, resume_after.as_deref(), page_size)` — `workload.pillar_priorities` is never consulted.

Proof the member is consumed nowhere else: grep for `pillar_priorities` across `src/` matches only `state.rs:2069` (the field), `state.rs:2600` (`pillar_priorities: Some(self.pillar_priorities.clone())` in the workload projection), `state.rs:4115` (`default_pillar_priorities`), `create_workload.rs:328-331,351,545` and `update_workload.rs:325,355-356`.

Test coverage: `tests/all/lens_review_improvements.rs:384-399` pins "the plan must report the identified risks in the lens's order" against `catalog()` (`:70-80`), which is `FRAMEWORK_PILLARS` (`:36-43`) — exactly the order `crate::state::default_pillar_priorities()` produces (`state.rs:4115-4120`, over `WELL_ARCHITECTED_PILLARS`) — and the workloads under test are built with `minimal_create`, which sends no `PillarPriorities`. `tests/all/workload_computed_members.rs:213-274` pins only what the member is REPORTED as, never its effect on the plan.

## Steps to Reproduce

1. CreateWorkload with `PillarPriorities: ["sustainability", "security", "operationalExcellence"]` and the `wellarchitected` lens
2. UpdateAnswer on one `sus-*` question and one `ops-*` question so both are rated HIGH (e.g. select no choice / mark a best practice unselected)
3. GET /workloads/{WorkloadId}/lensReviews/wellarchitected/improvements
4. Observe the `ops-*` item served before the `sus-*` item — the lens's catalog order (list_lens_review_improvements.rs:235-269) — although the workload's own PillarPriorities put sustainability first
5. Compare with src/state.rs:4111-4114, which states that the pillar priority order "is what orders the items in a review's improvement plan", and with the modelled WorkloadPillarPriorities documentation
