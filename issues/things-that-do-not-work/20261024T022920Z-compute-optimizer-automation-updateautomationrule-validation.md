Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d5cb6525aec300e57e039668d831e20baadf847e

# UpdateAutomationRule accepts criteria, schedules and empty action-type lists the service refuses

_Filed: 2026-10-24T02:29:20Z_
_Operation: UpdateAutomationRule | Class: validation_

> _Note: this issue was identified against the code base at commit `d5cb6525aec300e57e039668d831e20baadf847e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateAutomationRule` validates only `priority`'s decimal form, a non-empty `name`, and that the MERGED rule still has an `organizationConfiguration` when its type is `OrganizationRule` (src/ops/update_automation_rule.rs:223-241, 304-317). Every other member is applied to the stored rule unchecked, so an update can turn a perfectly valid rule into one the service could not have created:

* `criteria` (line 343-345) is stored with no call to `crate::state::criteria_violation`, so a condition with no `comparison`, with an empty `values` list, with an operator from the wrong family (e.g. `NumericGreaterThan` on `region`), or a `resourceTag` condition with no `key` is accepted with HTTP 200 — exactly the requests `ListAutomationRulePreview` refuses with `InvalidParameterValueException` (400). The documented cap "up to 20 conditions per filter criteria and 20 values per condition" is likewise unenforced.
* `recommendedActionTypes` (line 340-342) is replaced by whatever arrives, including an EMPTY list, leaving a rule that automates nothing.
* `schedule` (line 346-348) is replaced wholesale with no check on `scheduleExpression`, so `{}` (no expression) or a wildcard cron such as `"* * * * ? *"` is stored, contradicting "your cron expression must use specific values (not wildcards) for the minutes and hours fields".

Each of these is additionally observable as state corruption: the update also bumps `ruleRevision` and re-stamps `lastUpdatedTimestamp` (lines 352-354), so a request the real service would reject with 400 and no side effect instead advances the rule's concurrency token here.

Expected: `InvalidParameterValueException` (HTTP 400), the stored rule unchanged and its revision not bumped. Actual: HTTP 200, the invalid value persisted, revision incremented.

## Evidence

Spec/model: `UpdateAutomationRuleRequest` carries the same `criteria` (`Criteria`), `schedule` (`Schedule`) and `recommendedActionTypes` (`RecommendedActionTypeList`) shapes as the create (OPERATIONS.md, UpdateAutomationRule), and declares `InvalidParameterValueException` (400). `Criteria` doc: "You can specify up to 20 conditions per filter criteria and 20 values per condition"; `Schedule$scheduleExpression` doc: cron "must use specific values (not wildcards) for the minutes and hours fields".

Handler: src/ops/update_automation_rule.rs:223-241 is the whole input-value check (priority + name); the merge block at 325-351 assigns `criteria`, `schedule` and `recommended_action_types` verbatim; `crate::state::criteria_violation` (src/state.rs:1231) is never called from this file (its only callers are src/ops/list_automation_rule_preview.rs:218 and list_automation_rule_preview_summaries.rs:224).

Test coverage: `tests/all/automation_rule_lifecycle.rs:618-702` (`update_applies_only_what_it_carries_and_is_revision_checked`) exercises only VALID per-member updates (name, recommendedActionTypes, schedule, criteria at lines 642-657) plus the stale-revision refusal; no test sends a malformed criteria/schedule or an empty `recommendedActionTypes` to this operation.

## Steps to Reproduce

1. Enroll the account and create a valid rule; note its `ruleArn` and `ruleRevision`.
2. Call `UpdateAutomationRule` with that `ruleArn`/`ruleRevision` and `criteria: {"region": [{"comparison": "NumericGreaterThan", "values": ["us-east-1"]}]}` — expect HTTP 400 `InvalidParameterValueException` with the rule untouched; actual: HTTP 200, the criteria stored and `ruleRevision` bumped.
3. Call `GetAutomationRule` on the ARN — the malformed criteria is reported back as the rule's current criteria.
4. Call `ListAutomationRulePreview` with the same `criteria` — it IS refused with 400, showing the two paths disagree about the same value.
5. Call `UpdateAutomationRule` with `recommendedActionTypes: []`, then again with `schedule: {"scheduleExpression": "* * * * ? *"}` (each time with the current revision) — expect 400; actual: both accepted, stored, and the revision advanced each time.
