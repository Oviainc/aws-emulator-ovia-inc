Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# CreateAutomationRule stores criteria, schedules and action-type lists the service refuses

_Filed: 2026-10-24T02:29:20Z_
_Operation: CreateAutomationRule | Class: validation_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateAutomationRule`'s hand-written validation (`invalid_parameter`, src/ops/create_automation_rule.rs:402-425) checks only three things: a non-empty `name`, `organizationConfiguration` present for `OrganizationRule`, and the `priority` decimal form. Everything else the Smithy boundary cannot express is accepted and PERSISTED, so the store ends up holding rules the service itself could never run:

1. `criteria` is never validated. The crate already has the checker (`crate::state::criteria_violation`), but it is called ONLY from the two preview handlers. So a create with `criteria: {"region": [{"values": ["us-east-1"]}]}` (no `comparison`), `{"comparison": "StringEquals", "values": []}` (no values), a numeric operator on a string criterion (`region` + `NumericGreaterThan`), or a `resourceTag` condition with no `key` is accepted with HTTP 200 and stored — while `ListAutomationRulePreview` refuses exactly those requests with `InvalidParameterValueException` (400). The emulator is internally inconsistent: a rule can be created whose criteria can never be previewed or evaluated.
2. The documented criteria cap is nowhere enforced: `Criteria`'s doc states "You can specify up to 20 conditions per filter criteria and 20 values per condition"; a create carrying 50 conditions or 50 values per condition is accepted.
3. `schedule` is required but is accepted empty or with a wildcard cron. `Schedule$scheduleExpression` documents "You can schedule rules to run at most once per day. Your cron expression must use specific values (not wildcards) for the minutes and hours fields"; `schedule: {}` (no expression at all) and `schedule: {"scheduleExpression": "* * * * ? *"}` are both stored, producing a rule that either never runs or runs every minute.
4. `recommendedActionTypes` is required and names the action types the rule automates, but an EMPTY list is accepted and stored — again inconsistent with `ListAutomationRulePreview`, which refuses an empty `recommendedActionTypes` with 400.
5. Duplicate tag keys in `tags` are stored verbatim, so `ListTagsForResource`/`GetAutomationRule` later report two entries for one key; AWS tagging rejects a duplicate key in one request.

Expected: each of these is `InvalidParameterValueException` (HTTP 400) and leaves the store untouched. Actual: HTTP 200 and a stored rule.

## Evidence

Spec/model: `Criteria` doc — "You can specify up to 20 conditions per filter criteria and 20 values per condition"; `Schedule$scheduleExpression` doc — "You can schedule rules to run at most once per day. Your cron expression must use specific values (not wildcards) for the minutes and hours fields"; `CreateAutomationRuleRequest.recommendedActionTypes` is @required and `schedule` is @required (smithy_query shape=CreateAutomationRuleRequest/Criteria/Schedule). `InvalidParameterValueException` (400) is the declared error for a bad parameter value (OPERATIONS.md, CreateAutomationRule).

Handler: src/ops/create_automation_rule.rs:228-237 calls `invalid_parameter(&input)`, whose body (lines 402-425) checks only `name`, `organizationConfiguration` and `priority`; `input.criteria`, `input.schedule`, `input.recommended_action_types` and `input.tags` are moved straight into `StoredRule` at lines 343-347 with no inspection. `crate::state::criteria_violation` (src/state.rs:1231) exists and is referenced only by src/ops/list_automation_rule_preview.rs:218 and list_automation_rule_preview_summaries.rs:224; `condition_violation` (src/state.rs:1303) also does not count conditions or values, so the 20/20 cap is unenforced on every path.

Test coverage: `tests/all/automation_rule_lifecycle.rs:519-608` (`an_invalid_create_is_refused_and_stores_nothing`) enumerates exactly ten refused cases — empty name, name pattern, org-rule without configuration, three priority cases, status/ruleType enums, `executionWindowInMinutes` below range, missing required member — and asserts nothing about criteria, cron form, empty `recommendedActionTypes` or duplicate tag keys. By contrast `tests/all/automation_rule_preview.rs:968-1020` asserts the preview path refuses the empty action-type list and each malformed criteria condition, which pins the expected wording/status for the same values.

## Steps to Reproduce

1. Enroll the account: `UpdateEnrollmentConfiguration` with `status=Active`.
2. Call `CreateAutomationRule` with a valid `name`/`ruleType=AccountRule`/`status=Active`/`recommendedActionTypes=["UpgradeEbsVolumeType"]` and `criteria: {"region": [{"values": ["us-east-1"]}]}` (no `comparison`) — expect HTTP 400 `InvalidParameterValueException`; actual: HTTP 200 and a stored rule.
3. Call `ListAutomationRulePreview` with `ruleType=AccountRule`, the same `recommendedActionTypes` and the same `criteria` — it IS refused with 400, showing the two paths disagree about the same value.
4. Repeat the create with `criteria: {"region": [{"comparison": "NumericGreaterThan", "values": ["us-east-1"]}]}`, then with `{"comparison": "StringEquals", "values": []}`, then with a `resourceTag` condition carrying no `key` — all accepted.
5. Call `CreateAutomationRule` with `recommendedActionTypes: []` — expect 400; actual: 200, and `GetAutomationRule` reports a rule that automates nothing.
6. Call `CreateAutomationRule` with `schedule: {}` and again with `schedule: {"scheduleExpression": "* * * * ? *"}` — expect 400; actual: 200 and both stored.
7. Call `CreateAutomationRule` with `tags: [{"key": "env", "value": "a"}, {"key": "env", "value": "b"}]`, then `ListTagsForResource` on the new ARN — expect the create to be refused; actual: it is accepted (the second value wins) with no error.
