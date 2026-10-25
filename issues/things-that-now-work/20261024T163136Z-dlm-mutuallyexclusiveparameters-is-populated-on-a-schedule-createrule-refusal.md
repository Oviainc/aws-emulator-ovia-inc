Issue-Source: trace-sweep-divergence
Issue-Trace: dlm-create-policy-invalid-request-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dlm-create-policy-invalid-request-errors dlm-update-policy-error-paths
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm|CreateLifecyclePolicy|4xx->4xx|$.MutuallyExclusiveParameters
Issue-Key: dlm|UpdateLifecyclePolicy|4xx->4xx|$.MutuallyExclusiveParameters
Issue-Commit: 0831f46a299ca00fdb6555dd0d1d0166298aa2f3

# MutuallyExclusiveParameters is populated on a schedule CreateRule refusal AWS leaves unset

> _Note: this issue was identified against the code base at commit `0831f46a299ca00fdb6555dd0d1d0166298aa2f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLifecyclePolicy

## Description
This bundle has 2 causes, one per operation that routes a schedule's CreateRule refusal through the same shared validation helper. Both must be closed.

Cause 1 of 2: CreateLifecyclePolicy — a PolicyDetails whose Schedules element states BOTH a cron-based and an interval-based CreateRule (fixture dlm-create-policy-invalid-request-errors, step task#1).

AWS: Smithy: CreateLifecyclePolicy models error InvalidRequestException (errorKind client, httpError 400); InvalidRequestException members are Message, Code, RequiredParameters ("The request omitted one or more required parameters") and MutuallyExclusiveParameters ("The request included parameters that cannot be provided together"). CreateRule documents the either/or rule ("You must specify either CronExpression, or Interval, IntervalUnit, and Times"), but the model does not say which error members a given refusal populates. Recorded behavior: for this refusal AWS answers HTTP 400 InvalidRequestException with MutuallyExclusiveParameters ABSENT (null) — the refusal is carried by Message/Code alone. The recorded message text (e.g. naming the offending schedule) is an example, not a requirement; the absence of the MutuallyExclusiveParameters member is the wire fact.

Emulator: answers HTTP 400 InvalidRequestException (same status, same Code) but populates $.MutuallyExclusiveParameters as a list of wire paths — the replay reports expected None, got ["PolicyDetails.Schedules[1].CreateRule.CronExpression", "PolicyDetails.Schedules[1].CreateRule.Interval", "PolicyDetails.Schedules[1].CreateRule.IntervalUnit"].

Why: MutuallyExclusiveParameters is a structured, machine-readable member of the error shape, so a client branching on it sees a populated conflict list where AWS gives none. Code that treats a non-empty MutuallyExclusiveParameters as the authoritative conflict set (rendering it, retrying with those paths stripped, or asserting it is empty for this class of refusal) behaves differently against the emulator than against AWS, and a round-trip/diff test over the error payload fails on an extra member.

Fix: the shared CreateRule either/or validation that builds this InvalidRequestException — it should emit the refusal WITHOUT setting MutuallyExclusiveParameters for the cron-vs-interval conflict, i.e. build it the way the plain invalid_request path does rather than the parameters_cannot_be_combined path. Fixing the shared helper fixes both operations at once; do not fix only the CreateLifecyclePolicy handler.

Cause 2 of 2: UpdateLifecyclePolicy — the same both-forms-stated schedule on an update (fixture dlm-update-policy-error-paths, step task#6).

AWS: Smithy: UpdateLifecyclePolicy models the same InvalidRequestException (httpError 400) and takes the same PolicyDetails shape, so the same CreateRule either/or rule applies. Recorded behavior: HTTP 400 InvalidRequestException with MutuallyExclusiveParameters absent.

Emulator: HTTP 400 with $.MutuallyExclusiveParameters populated with the same three Schedules[1].CreateRule paths (expected None).

Why: same as cause 1 — an update's error payload carries a structured conflict list AWS does not send.

Fix: the same shared schedule-rule validation reached from the UpdateLifecyclePolicy handler; confirm the update path is covered and not just the create path.

Traces: dlm-create-policy-invalid-request-errors#task#1 (CreateLifecyclePolicy), dlm-update-policy-error-paths#task#6 (UpdateLifecyclePolicy).

No open dlm issue covers this: the three open dlm issues concern a DeleteLifecyclePolicy PolicyId-pattern error response, a GetLifecyclePolicies PolicyType projection, and region-scoping of the policy store — none of them touches how a schedule CreateRule conflict fills the InvalidRequestException members, and neither divergence was reported as owned.

## Evidence
Replay of trace set simparity-wire-ga-prod-v1: 69 steps, 67 passed, 2 mismatched (score 97.1%), emulator up.

Cause 1 — fixture dlm-create-policy-invalid-request-errors, step task#1, operation CreateLifecyclePolicy, verdict mismatch:
  expected_status 400, actual_status 400
  detail: $.MutuallyExclusiveParameters: type_mismatch (expected None, got ['PolicyDetails.Schedules[1].CreateRule.CronExpression', 'PolicyDetails.Schedules[1].CreateRule.Interval', 'PolicyDetails.Schedules[1].CreateRule.IntervalUnit'])
  divergence_key: service=dlm|CreateLifecyclePolicy|status=400->400|keys=$.MutuallyExclusiveParameters

Cause 2 — fixture dlm-update-policy-error-paths, step task#6, operation UpdateLifecyclePolicy, verdict mismatch:
  expected_status 400, actual_status 400
  detail: $.MutuallyExclusiveParameters: type_mismatch (expected None, got ['PolicyDetails.Schedules[1].CreateRule.CronExpression', 'PolicyDetails.Schedules[1].CreateRule.Interval', 'PolicyDetails.Schedules[1].CreateRule.IntervalUnit'])
  divergence_key: service=dlm|UpdateLifecyclePolicy|status=400->400|keys=$.MutuallyExclusiveParameters

Expected (recorded AWS): HTTP 400 InvalidRequestException with the MutuallyExclusiveParameters member absent.
Actual (emulator): HTTP 400 InvalidRequestException with MutuallyExclusiveParameters set to the three stated CreateRule member paths.

Model (services/service-com-amazonaws-dlm-dlm-20180112/smithy-model/com-amazonaws-dlm-dlm-20180112.json): InvalidRequestException { Message, Code, RequiredParameters: ParameterList, MutuallyExclusiveParameters: ParameterList }, httpError 400, bound to both CreateLifecyclePolicy and UpdateLifecyclePolicy.

Code path (both operations share it): services/service-com-amazonaws-dlm-dlm-20180112/src/ops/create_lifecycle_policy.rs line 299 calls crate::state::schedule_rule_violation; src/state.rs line 1179 schedule_rule_violation -> line 1204 create_rule_violation, whose (cron_based, interval_based) == (true, true) arm at line 1219 returns parameters_cannot_be_combined(...) with stated_members(...) (line 1324), and parameters_cannot_be_combined (line 763) sets mutually_exclusive_parameters: Some(parameters). The sibling constructor invalid_request (line 752) leaves it None.

## Steps to Reproduce
1. Boot the emulator and POST /policies (CreateLifecyclePolicy) with a custom (STANDARD) PolicyDetails whose Schedules list has a second element whose CreateRule states CronExpression AND Interval AND IntervalUnit together.
2. Observe HTTP 400 InvalidRequestException and inspect the response body: MutuallyExclusiveParameters is present and lists PolicyDetails.Schedules[1].CreateRule.CronExpression / .Interval / .IntervalUnit. AWS's recorded response for the same request omits MutuallyExclusiveParameters entirely.
3. Create a valid policy, then PATCH /policies/{PolicyId} (UpdateLifecyclePolicy) with the same both-forms-stated schedule and observe the same extra MutuallyExclusiveParameters member on the 400.
4. Replay fixture dlm-create-policy-invalid-request-errors step task#1 and fixture dlm-update-policy-error-paths step task#6 to confirm both; both must pass after the fix.
