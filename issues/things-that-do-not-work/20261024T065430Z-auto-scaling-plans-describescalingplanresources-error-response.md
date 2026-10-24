Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# Refuse DescribeScalingPlanResources for a plan name/version the service does not hold

_Filed: 2026-10-24T06:54:30Z_
_Operation: DescribeScalingPlanResources | Class: error-response_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A request naming a plan that does not exist (or an existing plan with a version other than the stored one) is answered HTTP 200 with `ScalingPlanResources: []` and no NextToken.

Expected (real service): the lookup by (ScalingPlanName, ScalingPlanVersion) fails and the call is refused with HTTP 400 carrying the service's not-found sentence — "No scaling plan found for scaling plan name: <name>, version: <version>" — modelled here as `ValidationException`, the operation's generic client-error shape (it does not bind `ObjectNotFoundException`). The crate's own state module states this rule: `plan_not_found_message` is documented as the text used by "Every operation that looks a plan up by name and version ... (`ObjectNotFoundException` where the operation models it, `ValidationException` where it does not)" — yet this handler, which does look a plan up, uses neither.

Actual (emulator): the miss short-circuits to an empty page, so a client cannot distinguish "this plan has no scalable resources" from "this plan does not exist", and a typo'd plan name looks like a successful describe.

## Evidence

Spec/grounding: `DescribeScalingPlanResources` binds `ValidationException` (HTTP 400) — OPERATIONS.md line 43 — and both required inputs identify a specific plan; the service's not-found wording is already implemented in src/state.rs `plan_not_found_message`, whose doc comment asserts it is answered by EVERY operation that looks a plan up, with ValidationException where no not-found shape is modelled. Handler: src/ops/describe_scaling_plan_resources.rs lines 171-181 return an empty page for a miss. Test coverage: tests/all/scaling_plan_lifecycle.rs (~lines 756-775) and tests/all/scaling_instruction_validation.rs (~lines 786-800) currently assert the empty-page behaviour, so fixing this requires updating those expectations.

## Steps to Reproduce

1. Create a plan named `known` (version 1) and let it settle to Active.
2. Call DescribeScalingPlanResources {"ScalingPlanName":"missing","ScalingPlanVersion":1} → emulator answers HTTP 200 {"ScalingPlanResources":[]}; expected HTTP 400 ValidationException "No scaling plan found for scaling plan name: missing, version: 1".
3. Call DescribeScalingPlanResources {"ScalingPlanName":"known","ScalingPlanVersion":2} → same empty 200, while DeleteScalingPlan/UpdateScalingPlan on the identical input correctly refuse with a 400 not-found.
