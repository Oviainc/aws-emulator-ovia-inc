Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 5bfe48ddd26ea9415acafc303bdb706a28d090a8

# UpdateRule never enforces the five-retention-rules-per-resource-tag-pair limit (ServiceQuotaExceededException is never raised)

_Filed: 2026-10-24T07:29:42Z_
_Operation: UpdateRule | Class: validation_

> _Note: this issue was identified against the code base at commit `5bfe48ddd26ea9415acafc303bdb706a28d090a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateRule can turn a rule into a tag-level rule, or change the resource tag pairs it selects on, by sending `ResourceTags`. Recycle Bin bounds how widely one resource tag pair may be re-used: `UpdateRuleRequest$ResourceTags` documents "You can add the same tag key and value pair to a maximum or five retention rules." UpdateRule is also one of only two operations in this service that model `ServiceQuotaExceededException` (HTTP 402) — and UpdateRule cannot touch the rule's own AWS tags (`Tags` is not an UpdateRule input), so the only quota an UpdateRule request can push past is this cross-rule resource-tag-pair limit.

Expected: an UpdateRule request whose `ResourceTags` would make a tag key/value pair the selector of a sixth retention rule is refused — HTTP 402 `ServiceQuotaExceededException` (Reason `SERVICE_QUOTA_EXCEEDED`), with the rule left unchanged (a later GetRule shows the OLD ResourceTags and the rule still `available`).

Actual: the handler applies the new resource tags unconditionally and answers HTTP 200, so an unbounded number of rules can share one tag pair. `src/ops/update_rule.rs` raises only `ResourceNotFoundException`, `ConflictException` (lock) and the `ResourceTags`+`ExcludeResourceTags` `ValidationException`; the `ServiceQuotaExceededException` variant of `UpdateRuleErrorModeled` is never constructed anywhere in the file, and `src/state.rs` keeps no index of resource tag pairs across rules (`insert_rule` indexes by identifier and ARN only), so the count cannot be computed today.

## Evidence

Spec: Smithy model `com.amazonaws.rbin#UpdateRuleRequest$ResourceTags` documentation — "You can add the same tag key and value pair to a maximum or five retention rules" (verified via smithy_query doc); UpdateRule declares `ServiceQuotaExceededException (HTTP 402)` with `ServiceQuotaExceededExceptionReason = [SERVICE_QUOTA_EXCEEDED]` among its errors (OPERATIONS.md "## UpdateRule").

Handler: `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/update_rule.rs` — `let merged_resource_tags = input.resource_tags.unwrap_or_else(|| rule.resource_tags.clone()); … rule.resource_tags = merged_resource_tags;` with no cross-rule check; grep of the file shows no use of `ServiceQuotaExceededException`. `src/state.rs` enforces only `MAX_RULES` (rules per Region) and `MAX_TAGS_PER_RULE` (the rule's own tags), both unrelated to resource tag pairs.

Test coverage: none. `grep -n 'ServiceQuotaExceeded|402' services/.../tests` matches only CreateRule's rules-per-Region quota (`retention_rule_lifecycle.rs::create_rule_is_refused_once_the_region_holds_its_quota_of_rules`) and the tags-per-rule quota on CreateRule/TagResource (`retention_rule_fidelity.rs`, `resource_tags.rs`). `retention_rule_lifecycle.rs::update_rule_applies_only_the_supplied_members_and_persists_them` asserts the happy path only; no test creates several rules sharing a resource tag pair.

Note: the sibling symptom on CreateRule is already filed (`20261024T020921Z-rbin-createrule-validation`, operation CreateRule); this is the same documented limit on the UpdateRule path, which that issue does not cover.

## Steps to Reproduce

1. POST /rules five times with body {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"ResourceTags":[{"ResourceTagKey":"retain","ResourceTagValue":"yes"}]} — five rules now share the tag pair retain=yes (all 201).
2. POST /rules once more with the SAME ResourceType and RetentionPeriod but ResourceTags [{"ResourceTagKey":"other","ResourceTagValue":"no"}] — 201; note its Identifier.
3. PATCH /rules/{Identifier} with body {"ResourceTags":[{"ResourceTagKey":"retain","ResourceTagValue":"yes"}]} — this would make retain=yes the selector of a sixth retention rule. Expect HTTP 402 ServiceQuotaExceededException (Reason SERVICE_QUOTA_EXCEEDED); actual HTTP 200 with the new ResourceTags applied.
4. GET /rules/{Identifier} — expected the rule still carrying other=no (the refused update changed nothing); actual it reports retain=yes, and POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","ResourceTags":[{"ResourceTagKey":"retain","ResourceTagValue":"yes"}]} returns six rules, a state the real service cannot reach.
