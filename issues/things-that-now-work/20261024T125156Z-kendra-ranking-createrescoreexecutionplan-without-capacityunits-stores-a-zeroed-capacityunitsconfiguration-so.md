Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kendra-ranking.create-plan-default-capacity-units-is-zero
Issue-Shape-Id: com.amazonaws.kendraranking#AWSKendraRerankingFrontendService
Issue-Key: kendra-ranking.create-plan-default-capacity-units-is-zero|run#7|AssertionError|assert "CapacityUnits" not in described, (
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# CreateRescoreExecutionPlan without CapacityUnits stores a zeroed CapacityUnitsConfiguration, so DescribeRescoreExecutionPlan echoes RescoreCapacityUnits: 0 instead of omitting the field

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRescoreExecutionPlan

## Description
One root cause.

Cause 1 of 1: a create that carries no CapacityUnits is persisted with a synthesized default capacity struct, so every reader of the plan reports a present-and-zero CapacityUnits where AWS reports no CapacityUnits at all. Failing check: kendra-ranking.create-plan-default-capacity-units-is-zero.

AWS: Smithy (services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/smithy-model/com-amazonaws-kendraranking-awskendrarerankingfrontendservice.json) models CreateRescoreExecutionPlanRequest$CapacityUnits as an OPTIONAL member targeting CapacityUnitsConfiguration, with NO @default and no @required; DescribeRescoreExecutionPlanResponse$CapacityUnits is likewise optional with no default. CapacityUnitsConfiguration$RescoreCapacityUnits is @required with @range min 0, so the struct cannot be serialized partially — it is either absent or carries a unit count. The CreateRescoreExecutionPlan docs state the default-capacity contract as an input-side one: "You are given a single capacity unit by default. If you want to use the default capacity, you don't set additional capacity units." Nothing in the model instructs the service to materialize a configuration the caller did not send. Recorded behavior (confirmed against real AWS in the check's own note) is that DescribeRescoreExecutionPlan OMITS the CapacityUnits key entirely for a plan created without an explicit CapacityUnits — the "capacity of zero indicates default" sentinel is surfaced as an absent key, not a present zeroed struct. HTTP status for both calls is 200; no error is involved.

Emulator: CreateRescoreExecutionPlan substitutes a fabricated configuration for the absent input member — `capacity_units: input.capacity_units.unwrap_or_else(crate::state::default_capacity_units)` — so the stored plan always HAS a capacity configuration. DescribeRescoreExecutionPlan then faithfully serializes it, and the replay observes `{'RescoreCapacityUnits': 0}` where the check asserts the key is absent: "AssertionError: expected CapacityUnits omitted for a plan created without an explicit CapacityUnits, got {'RescoreCapacityUnits': 0}". The handler's inline comment shows this is deliberate ("stored as such, so a reader reports RescoreCapacityUnits: 0 rather than nothing"), i.e. the defect is the modelling decision, not a serializer accident. The same stored value will also leak through any other reader of the plan record (ListRescoreExecutionPlans / UpdateRescoreExecutionPlan round-trips).

Why: a client cannot distinguish "plan running on default capacity" from "plan explicitly provisioned with zero additional units". Code that branches on key presence (`if 'CapacityUnits' in described`, or an SDK's optional/None check) takes the explicitly-configured branch for every default plan — e.g. an idempotent infrastructure reconciler reads back a zeroed struct, concludes the plan has drifted from its no-capacity-specified desired state, and issues a corrective UpdateRescoreExecutionPlan on every pass; a CloudFormation/Terraform-style differ reports a permanent phantom diff. Clients that echo the described configuration back into an update also convert an implicit default into an explicit pinned value.

Fix: the operation's state representation and its create handler. `capacity_units` on crate::state::RescoreExecutionPlan must be able to hold "not configured" (an Option) and CreateRescoreExecutionPlan must store the caller's CapacityUnits verbatim, including its absence, rather than filling it in from crate::state::default_capacity_units. The read paths (DescribeRescoreExecutionPlan, ListRescoreExecutionPlans) must then leave the optional output member unset when the plan has no configuration, so the key is omitted from the response body rather than serialized as a zeroed struct. UpdateRescoreExecutionPlan setting CapacityUnits is what should transition a plan from unconfigured to configured.

Traces: kendra-ranking.create-plan-default-capacity-units-is-zero. Emulator source: services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/ops/create_rescore_execution_plan.rs lines 258-264 (the unwrap_or_else default substitution and its justifying comment); the default value itself is crate::state::default_capacity_units in services/service-com-amazonaws-kendraranking-awskendrarerankingfrontendservice/src/state.rs. Not covered by any open kendra-ranking issue: 20261024T005004Z-kendra-ranking-listrescoreexecutionplans-wrong-behavior.md (region scoping of plans), 20261024T034227Z-kendra-ranking-deleterescoreexecutionplan-wrong-behavior.md (synchronous delete / DELETING status), 20261023T004037Z-kendra-ranking-rescore-does-not-boost-a-relevant-document-above.md and 20261024T023803Z-kendra-ranking-rescore-parameter-ignored.md (both Rescore data-plane scoring) all concern different behavior.

## Evidence
Check: kendra-ranking.create-plan-default-capacity-units-is-zero (verdict: mismatch).

Steps the check performs: create_rescore_execution_plan(Name=<unique>, Tags=[...]) with NO CapacityUnits member -> poll describe_rescore_execution_plan(Id=...) until Status is ACTIVE or FAILED.

Expected (real AWS, 200 OK): the DescribeRescoreExecutionPlan response body contains NO CapacityUnits key, because the member is optional and unset for a plan created without an explicit CapacityUnits.

Actual (emulator, 200 OK): the response body contains CapacityUnits = {'RescoreCapacityUnits': 0}.

Replay assertion:
  File "check_kendra_ranking_create_plan_default_capacity_units_is_zero.py", line 32, in run
    assert "CapacityUnits" not in described
  AssertionError: expected CapacityUnits omitted for a plan created without an explicit CapacityUnits, got {'RescoreCapacityUnits': 0}

Status is correct on both calls (200) and Status reached ACTIVE as expected; the divergence is purely the presence of an optional output member. Model evidence: CreateRescoreExecutionPlanRequest$CapacityUnits and DescribeRescoreExecutionPlanResponse$CapacityUnits are optional with no @default; CapacityUnitsConfiguration$RescoreCapacityUnits is @required, @range min 0. Source evidence: create_rescore_execution_plan.rs stores `input.capacity_units.unwrap_or_else(crate::state::default_capacity_units)`.

## Steps to Reproduce
1. Point a botocore kendra-ranking client at the emulator.
2. plan_id = kr.create_rescore_execution_plan(Name='sim2real-krank-example', Tags=[{'Key': 'sim2real-env', 'Value': 'behavioral'}])['Id']  — note that CapacityUnits is deliberately NOT passed.
3. Poll described = kr.describe_rescore_execution_plan(Id=plan_id) until described['Status'] == 'ACTIVE'.
4. Inspect the response: assert 'CapacityUnits' not in described.
   Emulator: the key is present as {'RescoreCapacityUnits': 0} and the assertion fails.
   AWS: the key is absent from the response.
5. Optionally also call kr.list_rescore_execution_plans() and confirm the same stored zeroed struct is reported there.
6. Cleanup: kr.delete_rescore_execution_plan(Id=plan_id).
