Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 63ca14555026a1bc3ce90b8f8423be0f1109f448

# CreateRule accepts a RetentionPeriod the requested ResourceType does not support

_Filed: 2026-10-26T05:14:27Z_
_Operation: CreateRule | Class: validation_

> _Note: this issue was identified against the code base at commit `63ca14555026a1bc3ce90b8f8423be0f1109f448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateRuleRequest` carries both the `ResourceType` to protect and the `RetentionPeriod` to retain for, and the model documents the retention period as depending on that type: `RetentionPeriod$RetentionPeriodValue` — "The supported retention periods are: EBS volumes: 1 - 7 days; EBS snapshots and EBS-backed AMIs: 1 - 365 days". The member's `@range` is the looser {min:1, max:3650}, which is the union of the two, so the per-type bound is a rule only the handler can apply.

The handler applies nothing: it stores `input.retention_period` verbatim with no reference to `input.resource_type`, so `CreateRule` mints EBS_VOLUME rules with a 3650-day retention period and EBS_SNAPSHOT / EC2_IMAGE rules with anything up to 3650 days, and a later `GetRule`/`ListRules` reports those rules as real, `available` retention rules.

Expected: `CreateRule` with `ResourceType=EBS_VOLUME` and `RetentionPeriodValue > 7` -> HTTP 400 `ValidationException` (Message + Reason=INVALID_PARAMETER_VALUE, the envelope `crate::state::member_validation_failed` already produces) and no rule minted; `ResourceType=EBS_SNAPSHOT` or `EC2_IMAGE` with `RetentionPeriodValue > 365` -> HTTP 400; the documented maxima (7 and 365) and everything below them still accepted with HTTP 201. Actual: HTTP 201 for every value the generic `@range` admits.

This is the CreateRule door of the same coupling the open issue `20261024T204824Z-rbin-updaterule-validation--r2.md` reports for `UpdateRule`; that issue is written entirely against `update_rule.rs` (where the type cannot change), and the create path — the only place `ResourceType` is ever chosen — is unguarded as well, so a rule the service cannot hold can be minted outright rather than only patched into existence.

## Evidence

Spec (smithy_query(query="doc", shape="RetentionPeriod", member="RetentionPeriodValue")): "The period value for which the retention rule is to retain resources, measured in days. The supported retention periods are: EBS volumes: 1 - 7 days / EBS snapshots and EBS-backed AMIs: 1 - 365 days". The member itself carries only `range {min: 1, max: 3650}` (smithy_query(query="shape", shape="RetentionPeriod")), and `CreateRuleRequest$ResourceType` is `@required` ("The resource type to be retained by the retention rule"), so the request always names the type the bound must be read against. `CreateRule` models `ValidationException (HTTP 400)` — "One or more of the parameters in the request is not valid" (services/service-com-amazonaws-rbin-amazonrecyclebin/OPERATIONS.md:31).

Handler: services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs:303-321 builds the record with
```
        let mut rule = crate::state::Rule {
            identifier: identifier.clone(),
            rule_arn,
            description: input.description,
            resource_type: input.resource_type,
            retention_period: input.retention_period,
```
— `input.retention_period` is stored unchecked, and `input.resource_type` is never consulted. The guards above it (create_rule.rs:217-227 ResourceTags/ExcludeResourceTags, :233-242 lock configuration, :250-263 tags quota, :269-280 rules-per-Region quota, :286-291 resource-tag-pair quota) cover everything except this coupling, and `src/state.rs` has no retention-period helper at all (grep for `retention` in state.rs yields only the `Rule.retention_period` field and `RetentionPeriod` imports).

Test coverage encodes the gap rather than the rule:
- tests/all/retention_rule_lifecycle.rs:172-179 case "region-level rule at the maximum retention period" sends `"ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(RETENTION_VALUE_MAX)` with `RETENTION_VALUE_MAX: i64 = 3650` (retention_rule_lifecycle.rs:33-35, commented "Source: smithy_query(query=\"shape\", shape=\"RetentionPeriod\") — range {min: 1, max: 3650}") and asserts HTTP 201 (line 242-245) — 3650 days for EBS volumes, 521x the documented 7-day maximum.
- tests/all/retention_rule_lifecycle.rs:201-208 case "locked rule at the minimum unlock delay" sends `"ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(366)` and also expects 201.
- tests/all/retention_rule_lifecycle.rs:373-397 `retention_period_is_refused_outside_its_published_range` only exercises the generic `@range` ("Each published bound is accepted, and the value just past it is refused": rows `("maximum bound", RETENTION_VALUE_MAX, 201)` / `("one above the maximum", RETENTION_VALUE_MAX + 1, 400)`), so the only "published" figure any test or comment in the crate cites is the model's 3650 `@range` endpoint — nothing in the crate states a reason for ignoring the per-resource-type periods the member documentation publishes.

## Steps to Reproduce

1. POST /rules with {"ResourceType":"EBS_VOLUME","RetentionPeriod":{"RetentionPeriodValue":3650,"RetentionPeriodUnit":"DAYS"}} — expect HTTP 400 ValidationException (EBS volumes support 1 - 7 days); actual HTTP 201 with a minted Identifier and RetentionPeriodValue 3650.
2. GET /rules/{Identifier} — expect 404 (nothing should have been minted); actual HTTP 200 reporting the 3650-day EBS_VOLUME rule, which settles to Status "available".
3. POST /rules with {"ResourceType":"EBS_VOLUME","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}} — must still be HTTP 201, so the documented 7-day maximum stays accepted.
4. POST /rules with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":366,"RetentionPeriodUnit":"DAYS"}} — expect HTTP 400 (snapshots and EBS-backed AMIs support 1 - 365 days); actual HTTP 201.
5. POST /rules with {"ResourceType":"EC2_IMAGE","RetentionPeriod":{"RetentionPeriodValue":365,"RetentionPeriodUnit":"DAYS"}} — must still be HTTP 201, so the bound is read off the requested resource type rather than tightened for every type.
