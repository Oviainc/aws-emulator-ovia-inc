Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# UpdateWorkload accepts AwsRegions=[] and NonAwsRegions=[], leaving a stored workload that names no Region at all

_Filed: 2026-10-24T15:17:52Z_
_Operation: UpdateWorkload | Class: validation_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateWorkload` enforces the documented invariant that a workload always names at least one Region — "Either `AwsRegions`, `NonAwsRegions`, or both must be specified" — and the emulator refuses a create that names none with the modelled `ValidationException` whose `Fields` names both members (`state::either_regions_exception`).

`UpdateWorkload` applies no such rule. It validates each supplied Region string against the known-Region list, then assigns whatever list the request carried: `if let Some(aws_regions) = input.aws_regions { record.aws_regions = aws_regions; }` and the same for `non_aws_regions`. `WorkloadAwsRegions`/`WorkloadNonAwsRegions` publish no `@length(min: 1)`, so an empty list passes the boundary walk, and a single `PATCH /workloads/{WorkloadId}` carrying `{"AwsRegions": [], "NonAwsRegions": []}` leaves the store holding a workload with no Region at all.

Expected: the update is refused with the same `ValidationException` the create words for the same situation (the Region rule is an invariant of the resource, not only of its creation), and the stored workload keeps the Regions it had. Actual: HTTP 200, and `GetWorkload`/`ListWorkloads` then report a workload whose `AwsRegions` and `NonAwsRegions` are both empty — a state the create path would have rejected and that no real workload can be in.

## Evidence

Spec: `smithy_query(query="doc", shape="CreateWorkload")` carries the `@important` rule "Either AwsRegions, NonAwsRegions, or both must be specified when creating a workload"; `UpdateWorkload` takes the same two members (`smithy_query(query="operation", operation="UpdateWorkload")`, `AwsRegions` len ..50, `NonAwsRegions` len ..5, neither with a minimum) and models `ValidationException` (HTTP 400).

Emulator: `src/ops/update_workload.rs` lines 226-230 check only `unknown_aws_region_exception` for the supplied values, and lines 294-299 assign both lists unconditionally; contrast `src/ops/create_workload.rs` lines 250-261, which refuse the empty/empty case with `crate::state::either_regions_exception()`.

Test coverage: `tests/all/validation_envelopes.rs` `the_regions_rule_names_every_member_that_satisfies_it` (lines 336-379) exercises the rule for CREATE only (`client.create(..)`), with rows `both-empty`, `aws-empty-non-aws-omitted`, `non-aws-empty-aws-omitted`; no test sends an empty Region list to UpdateWorkload, so nothing pins the current 200.

## Steps to Reproduce

1. Boot the wellarchitected emulator
2. POST /workloads (CreateWorkload) with WorkloadName="regions-update", Description, Environment=PRODUCTION, ReviewOwner, AwsRegions=["us-west-2"], Lenses=["wellarchitected"], ClientRequestToken=tok-1; note the WorkloadId
3. PATCH /workloads/{WorkloadId} (UpdateWorkload) with body {"AwsRegions": [], "NonAwsRegions": []}
4. Observe HTTP 200 instead of a 400 ValidationException naming AwsRegions and NonAwsRegions
5. GET /workloads/{WorkloadId} and observe the stored workload now reports no Regions at all (both lists empty), a state CreateWorkload refuses
