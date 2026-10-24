Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# UpdateRule accepts a RetentionPeriod the rule's stored ResourceType does not support

_Filed: 2026-10-24T20:48:24Z_
_Operation: UpdateRule | Class: validation_

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateRule` can change a rule's `RetentionPeriod` but explicitly cannot change its `ResourceType` (`UpdateRuleRequest$ResourceType`: "This parameter is currently not supported. You can't update a retention rule's resource type after creation"). The new retention period therefore has to satisfy the limit for the rule's ALREADY-STORED resource type: `RetentionPeriod$RetentionPeriodValue` — "EBS volumes: 1 - 7 days; EBS snapshots and EBS-backed AMIs: 1 - 365 days".

Expected:
* `UpdateRule` on an `EBS_VOLUME` rule with `RetentionPeriod.RetentionPeriodValue > 7` -> HTTP 400 `ValidationException` (Message + Reason=INVALID_PARAMETER_VALUE), and the stored rule is left untouched (still `available`, still carrying its old retention period).
* `UpdateRule` on an `EBS_SNAPSHOT` / `EC2_IMAGE` rule with `RetentionPeriodValue > 365` -> HTTP 400 `ValidationException`, rule untouched.

Actual: HTTP 200. The handler writes `rule.retention_period = retention_period` whenever the member is present, checking nothing beyond the model's generic `@range` 1..3650, then calls `rule.begin_pending(now)` — so the rule is mutated into a state the real service cannot hold (and is additionally knocked back to `pending` by a request that should have been refused before any write).

This is a distinct gap from the already-fixed UpdateRule/validation issue `20261024T072942Z-rbin-updaterule-validation.md`, which was about the five-retention-rules-per-resource-tag-pair quota; that check is now present in the handler, the retention-period/resource-type coupling is not.

## Evidence

Spec: Smithy model `com.amazonaws.rbin#RetentionPeriod$RetentionPeriodValue` documentation — "The supported retention periods are: EBS volumes: 1 - 7 days; EBS snapshots and EBS-backed AMIs: 1 - 365 days" (the member's `@range` is the looser {min:1, max:3650}); `com.amazonaws.rbin#UpdateRuleRequest$ResourceType` documentation — "You can't update a retention rule's resource type after creation", so the stored type is the one the bound must be read against. UpdateRule models `ValidationException (400)`.

Handler: `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/update_rule.rs::handle` — `if let Some(retention_period) = input.retention_period { rule.retention_period = retention_period; }` with no reference to `rule.resource_type`; the guards above it cover only lock protection, the ResourceTags/ExcludeResourceTags combination and the resource-tag-pair quota. No retention-period helper exists in `src/state.rs`.

Test coverage: none asserts the documented limits, and `tests/all/retention_rule_lifecycle.rs::update_rule_applies_only_the_supplied_members_and_persists_them` encodes the gap — it patches an `EBS_SNAPSHOT` rule to `RetentionPeriodValue: 3650` (row "retention period and description together") and expects HTTP 200, ten times the documented 365-day snapshot maximum.

## Steps to Reproduce

1. POST /rules with {"ResourceType":"EBS_VOLUME","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} — HTTP 201; note the Identifier.
2. PATCH /rules/{Identifier} with {"RetentionPeriod":{"RetentionPeriodValue":30,"RetentionPeriodUnit":"DAYS"}} — expect HTTP 400 ValidationException (the rule's stored ResourceType is EBS_VOLUME, limit 7 days); actual HTTP 200 with RetentionPeriodValue 30.
3. GET /rules/{Identifier} — expect the original 7-day retention period to have survived the refusal; actual the rule reports 30 days and Status "pending".
4. POST /rules with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,…}}, then PATCH /rules/{Identifier} with {"RetentionPeriod":{"RetentionPeriodValue":3650,"RetentionPeriodUnit":"DAYS"}} — expect HTTP 400 (snapshots support 1-365 days); actual HTTP 200.
5. PATCH the same EBS_SNAPSHOT rule with {"RetentionPeriod":{"RetentionPeriodValue":365,…}} — must still be accepted with HTTP 200, so the bound is read off the stored resource type rather than tightened for everyone.
