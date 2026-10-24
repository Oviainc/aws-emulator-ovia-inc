Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# CreateRule accepts a LockConfiguration for rules Recycle Bin cannot lock (and ignores the five-rules-per-tag-pair limit)

_Filed: 2026-10-24T02:09:21Z_
_Operation: CreateRule | Class: validation_

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Recycle Bin can only lock Region-level retention rules that have no exclusion tags. The service states the invariant on LockRule: "You can't lock tag-level retention rules, or Region-level retention rules that have exclusion tags." A CreateRule request that combines `LockConfiguration` with `ResourceTags` (which makes the rule tag-level) or with `ExcludeResourceTags` (a Region-level rule with exclusions) therefore cannot be served and is refused by the real service with a 400 `ValidationException` (the only parameter-fault error CreateRule models).

The emulator accepts it. `src/ops/create_rule.rs` derives the lock state purely from the PRESENCE of `LockConfiguration` (`let lock_state = input.lock_configuration.as_ref().map(|_| "locked".to_string());`) and performs only one member-combination check — `ResourceTags` together with `ExcludeResourceTags`. So:

* Expected: HTTP 400 `ValidationException` (Message + Reason=INVALID_PARAMETER_VALUE) for `CreateRule{ResourceTags:[…], LockConfiguration:{…}}` and for `CreateRule{ExcludeResourceTags:[…], LockConfiguration:{…}}`.
* Actual: HTTP 201 with `LockState: "locked"`, and the rule is persisted in a state the real service can never produce. Worse, that rule is now protected by `Rule::is_lock_protected`, so `UpdateRule` and `DeleteRule` answer 409 `ConflictException` for it forever (until unlocked + 7-day delay), while `LockRule` on an identically-shaped rule is correctly refused with 409 — the emulator contradicts its own invariant depending on which door the lock came through.

Second symptom of the same class on the same operation: the documented hard limit on `ResourceTags` — "You can add the same tag key and value pair to a maximum or five retention rules" (CreateRuleRequest$ResourceTags docs) — is not enforced anywhere. A sixth tag-level rule naming the same ResourceTag key/value pair is accepted with 201; the real service refuses it (CreateRule models `ServiceQuotaExceededException` 402 as well as `ValidationException` 400 for this class of refusal). The emulator only bounds total rules per Region (`MAX_RULES`) and tags per rule (`MAX_TAGS_PER_RULE`).

## Evidence

Spec/semantics: Smithy model `com.amazonaws.rbin#LockRule` documentation — "Locks a Region-level retention rule… You can't lock tag-level retention rules, or Region-level retention rules that have exclusion tags"; `com.amazonaws.rbin#CreateRuleRequest$ResourceTags` documentation — "You can add the same tag key and value pair to a maximum or five retention rules"; CreateRule models `ValidationException (400)` and `ServiceQuotaExceededException (402)`.

Handler: `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs` — the only member-combination guard is the ResourceTags+ExcludeResourceTags check (`if !resource_tags.is_empty() && !exclude_resource_tags.is_empty()`), and `lock_state` is then set to `locked` for ANY request carrying `LockConfiguration`, with no inspection of `resource_tags` / `exclude_resource_tags`; no cross-rule tag-pair counting exists in `src/state.rs` (`insert_rule` indexes by identifier/ARN only).

Contrast: `src/ops/lock_rule.rs` enforces the same invariant correctly for LockRule, returning `ConflictException` with `BlockingCondition::TagLevelRule` / `RuleWithExclusionTags`.

Test coverage: none. `tests/all/rule_lock_transitions.rs::lock_rule_is_refused_for_every_rule_that_cannot_be_locked` creates the tag-level and exclusion-tag rules WITHOUT a lock configuration and only exercises LockRule; `tests/all/retention_rule_lifecycle.rs::create_cases()` has a "locked rule at the minimum unlock delay" case with neither ResourceTags nor ExcludeResourceTags, and `exclusion_tags_cannot_be_combined_with_resource_tags` covers only the other combination. No test creates a rule with LockConfiguration plus ResourceTags/ExcludeResourceTags, and no test counts rules sharing a resource tag pair.

## Steps to Reproduce

1. POST /rules with body {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"ResourceTags":[{"ResourceTagKey":"retain","ResourceTagValue":"yes"}],"LockConfiguration":{"UnlockDelay":{"UnlockDelayValue":7,"UnlockDelayUnit":"DAYS"}}} — expect HTTP 400 ValidationException (a tag-level rule cannot be locked); actual HTTP 201 with "LockState":"locked".
2. GET /rules/{Identifier} for that rule — the emulator reports a locked tag-level rule, a state the real service cannot hold.
3. DELETE /rules/{Identifier} — expect 204 (the rule should never have been locked); actual 409 ConflictException "because it is locked", so the rule cannot be removed.
4. PATCH /rules/{Identifier}/lock on a tag-level rule created WITHOUT a lock configuration — correctly refused with 409, showing the inconsistency with step 1.
5. Repeat step 1 with "ExcludeResourceTags":[{"ResourceTagKey":"scratch"}] instead of ResourceTags — expect HTTP 400; actual HTTP 201 with "LockState":"locked".
6. Create six rules of the same ResourceType each with ResourceTags [{"ResourceTagKey":"retain","ResourceTagValue":"yes"}] — expect the sixth to be refused (same tag key/value pair in at most five retention rules); actual all six return HTTP 201.
