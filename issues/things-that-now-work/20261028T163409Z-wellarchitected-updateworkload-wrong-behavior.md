Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ba27fe699c954bc2ce7c4c631fb862b19a4228d3

# UpdateWorkload stores PillarPriorities: [] verbatim, leaving a workload with no pillar order — the state CreateWorkload is written never to produce

_Filed: 2026-10-28T16:34:09Z_
_Operation: UpdateWorkload | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ba27fe699c954bc2ce7c4c631fb862b19a4228d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkload` treats an explicitly EMPTY `PillarPriorities` list the same as an absent one and records the framework's own pillar order, on the stated ground that "a workload is always reviewed in some pillar order" — an invariant of the workload, not a rule about one request. `UpdateWorkload` applies no such normalisation: `PillarPriorities: []` is `Some(vec![])`, so the empty list is assigned straight onto the stored record and every later `GetWorkload` / `UpdateWorkload` / milestone read of that workload reports `PillarPriorities: []`.

So a workload that no create could have produced is reachable by updating one that could — the exact failure mode the handler's own Regions block, a few lines above, exists to prevent ("a workload no create could have produced cannot be reached by updating one that could"). The Regions invariant is judged on the record the merge WOULD leave behind; the pillar-order invariant is not judged at all.

`WorkloadPillarPriorities` carries no `@length`, so the boundary validator passes an empty list through to the handler.

## Evidence

Side 1 — the create normalises empty to the framework order, with the invariant stated: `src/ops/create_workload.rs:324-331` — "// \"The priorities of the pillars, which are used to order items in the // improvement plan\" (`PillarPriorities` doc): a workload is always reviewed in // some pillar order, so a request that names none is created with the // framework's own pillar order rather than with no priorities at all." followed by `let pillar_priorities = match input.pillar_priorities { Some(priorities) if !priorities.is_empty() => priorities, _ => crate::state::default_pillar_priorities(), };`

Side 2 — the update stores whatever the request carried: `src/ops/update_workload.rs:355-357` — `if let Some(pillar_priorities) = input.pillar_priorities { record.pillar_priorities = pillar_priorities; }` — no empty-list arm, and no merge-time invariant check for this member.

Side 3 — the same handler DOES judge the sibling invariant on the merged record, and says why: `src/ops/update_workload.rs:254-271` — "// \"Either `AwsRegions`, `NonAwsRegions`, or both must be specified\" is an // invariant of the workload, not a rule about one request... A merge is therefore judged on the // record it WOULD leave behind ... and refused before // anything is mutated, so a workload no create could have produced cannot be // reached by updating one that could."

Side 4 — the crate's own test states the invariant and pins the empty-list case on the create path only: `tests/all/workload_computed_members.rs:209-218` — "/// A workload is always reviewed in some pillar order: one created without /// `PillarPriorities` gets the framework's own pillar order, and one that names its /// priorities keeps exactly what it named." with the row `// An explicitly empty list names no priority order either.` / `("empty", Some(json!([])), framework_pillar_order())`. The test exercises `client.create(request)` (`:256`) and then only reads the workload back; no row sends `PillarPriorities` through `UpdateWorkload`, so normalising the update leaves every existing row green.

The boundary does not refuse the empty list: `smithy_query(query="shape", shape="WorkloadPillarPriorities")` returns `{"type": "list", "member": {"target": "PillarId", "length": {"min": 1, "max": 64}}}` — a length bound on the MEMBER, none on the list.

## Steps to Reproduce

1. CreateWorkload without `PillarPriorities` and confirm GetWorkload reports the six framework pillars (tests/all/workload_computed_members.rs:213-274 pins this)
2. PATCH /workloads/{WorkloadId} with body `{"PillarPriorities": []}` — answered 200
3. GET /workloads/{WorkloadId} — the workload now reports `"PillarPriorities": []`, a workload reviewed in no pillar order
4. Compare with src/ops/create_workload.rs:324-331, which refuses to produce that record for the identical request body, and with src/ops/update_workload.rs:254-271, which judges the sibling Regions invariant on the merged record for exactly this reason
