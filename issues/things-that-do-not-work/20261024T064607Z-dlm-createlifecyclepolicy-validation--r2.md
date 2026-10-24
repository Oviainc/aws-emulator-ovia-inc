Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 5dc70b01fea90015c733b45f7727cf9a955e7fe2

# Enforce the conditionally required policy configuration members a create must carry

_Filed: 2026-10-24T06:46:07Z_
_Operation: CreateLifecyclePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `5dc70b01fea90015c733b45f7727cf9a955e7fe2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A create is accepted with HTTP 200 (and a policy is persisted) for configurations Amazon Data Lifecycle Manager refuses outright. This is a different class of refusal from the already-fixed interval-RANGE issue (that one was about the magnitude of CreateInterval/RetainInterval/CreateRule$Interval); this is about members that are conditionally REQUIRED and about the either/or forms of CreateRule and RetainRule.

Expected (HTTP 400 `InvalidRequestException`, `Code` `InvalidParameter`, with `RequiredParameters` naming what was omitted):
* a request that states NEITHER `DefaultPolicy` nor `PolicyDetails` — nothing determines what the policy would do, so there is no policy to create;
* a custom snapshot/AMI policy whose `PolicyDetails` omits `ResourceTypes`, `TargetTags` or `Schedules` (`Schedules` is documented as carrying "one mandatory schedule and up to three optional schedules");
* a schedule whose `CreateRule` states NEITHER form of "when to create" (no `CronExpression`, and no `Interval`/`IntervalUnit`/`Times`), or the interval form without `IntervalUnit`;
* a schedule whose `RetainRule` states neither `Count` nor `Interval`+`IntervalUnit`, or states a count AND a time interval (the two retention modes are alternatives).

Actual: every one of these is stored and then reported by `GetLifecyclePolicy` / `GetLifecyclePolicies`. A create carrying only `ExecutionRoleArn`, `Description` and `State` mints a policy id and persists a record with no configuration at all; the summary projection then reports a fabricated `PolicyType` of `EBS_SNAPSHOT_MANAGEMENT` for it (`state.rs` `policy_type()`).

## Evidence

Spec: `PolicyDetails$Schedules` doc (smithy_query doc shape=PolicyDetails member=Schedules) — "A policy can have up to four schedules—one mandatory schedule and up to three optional schedules."; `CreateRule` doc — "You must specify either CronExpression, or Interval, IntervalUnit, and Times."; `RetainRule` doc — "You can retain snapshots based on either a count or a time interval. Count-based retention: You must specify Count ... Age-based retention: You must specify Interval and IntervalUnit."; `ResourceTypes`/`TargetTags` are documented "[Custom snapshot and AMI policies only]" members of the configuration a custom policy is built from. The operation models `InvalidRequestException` (HTTP 400) whose `RequiredParameters` member exists precisely for "the request omitted one or more required parameters" — members the model itself cannot mark `@required` because a default policy states them elsewhere.
Handler: `src/ops/create_lifecycle_policy.rs` `handle` (lines 183-375) runs exactly six checks before `store.policies.insert` — default-only duplication (211-228), default-only parameters on a custom policy (234-248), `Cadence::violation` (257-276), `schedule_with_conflicting_create_rule` (283-289), `schedule_with_unsupported_create_interval` (294-301), reserved `aws:` tag key (305-317), plus the store quota. `input.policy_details` is never required and its contents are never checked for completeness; `state.rs` `create_rule_states_both_forms` (lines 998-1003) only catches a rule that states BOTH forms, never one that states neither; nothing anywhere inspects `RetainRule`.
Tests: every fixture in `tests/all/*.rs` sends a complete `PolicyDetails` (`ResourceTypes` + `TargetTags` + `Schedules` with `CreateRule` and `RetainRule` — see the grep hits in `lifecycle_policy_crud.rs:246-275`, `error_paths.rs:143-168`, `interval_validation.rs:535-560`). No test sends an absent/incomplete `PolicyDetails`, an empty `CreateRule`, or a `RetainRule` with both or neither retention mode, so no test asserts these refusals.

## Steps to Reproduce

1. POST /policies with {"ExecutionRoleArn": "<emulator-account role ARN>", "Description": "no configuration", "State": "ENABLED"} — no PolicyDetails, no DefaultPolicy
2. Observe HTTP 200 with a minted PolicyId instead of 400 InvalidRequestException (Code InvalidParameter, RequiredParameters naming PolicyDetails)
3. GET /policies/{PolicyId} and observe a stored policy with no PolicyDetails, and GET /policies reporting it with PolicyType EBS_SNAPSHOT_MANAGEMENT
4. POST /policies with PolicyDetails {"PolicyType": "EBS_SNAPSHOT_MANAGEMENT"} only (no ResourceTypes, TargetTags or Schedules) and observe HTTP 200
5. POST /policies with PolicyDetails whose Schedules is [{"Name": "daily", "CreateRule": {}, "RetainRule": {}}] and observe HTTP 200 instead of a 400 naming CreateRule (neither cron nor interval form) and RetainRule (neither Count nor Interval+IntervalUnit)
6. POST /policies with Schedules[0].RetainRule = {"Count": 3, "Interval": 7, "IntervalUnit": "DAYS"} and observe HTTP 200 instead of a 400 for the two mutually exclusive retention modes
