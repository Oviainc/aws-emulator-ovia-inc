Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 11fd98189f90040dc28179001c4adbbf7cfc6f77

# The "resource tags and exclusion tags cannot both be set" rule is written out twice, once in CreateRule and once in UpdateRule, instead of once in crate::state

_Filed: 2026-10-27T04:17:02Z_
_Operation: UpdateRule | Class: maintainability_

> _Note: this issue was identified against the code base at commit `11fd98189f90040dc28179001c4adbbf7cfc6f77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate single-homes every invariant that more than one operation can arrive through: `crate::state::unlockable_rule` owns "which rules a lock can hold on" for CreateRule, LockRule and UpdateRule; `crate::state::retention_period_unsupported` owns the per-resource-type retention bound for CreateRule and UpdateRule; `crate::state::tag_quota_exceeded` owns the tags-per-rule refusal for CreateRule and TagResource. Each of those helpers' doc comments states explicitly that it lives in one place so two doors cannot disagree.

The one invariant that is NOT single-homed is the mutual exclusion of `ResourceTags` and `ExcludeResourceTags`. The predicate (`both lists non-empty`), the member it names and the requirement it states are written out inline in `src/ops/create_rule.rs` and again in `src/ops/update_rule.rs` — only the two string constants it interpolates (`EXCLUDE_RESOURCE_TAGS_MEMBER`, `EXCLUDE_RESOURCE_TAGS_REQUIREMENT`) live in `src/state.rs`. The two copies therefore can drift, and only the CreateRule copy has a direct test, so a change to one would not be caught by the suite.

The fix is the shape the crate already uses for the sibling rules: a `crate::state` helper returning `Option<ValidationException>` (exactly like `retention_period_unsupported`) called from both handlers. Every response byte and every existing test stays unchanged — the refusal body is already composed by the shared `member_validation_failed` with the shared member/requirement constants, so hoisting the predicate changes no wire output.

## Evidence

Copy 1 — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs:214-227`:

    // The model cannot express this: `ResourceTags` makes the rule tag-level, and
    // `ExcludeResourceTags` is "[Region-level retention rules only] ... You can't
    // specify exclusion tags for tag-level retention rules".
    if !resource_tags.is_empty() && !exclude_resource_tags.is_empty() {
        // A rule the MODEL cannot express, stated in the same envelope a modelled
        // constraint takes: the member that cannot hold, and what it must satisfy.
        return Err(CreateRuleErrorModeled::ValidationException(
            crate::state::member_validation_failed(
                crate::state::EXCLUDE_RESOURCE_TAGS_MEMBER,
                crate::state::EXCLUDE_RESOURCE_TAGS_REQUIREMENT,
            ),
        )
        .into());
    }

Copy 2 — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/update_rule.rs:266-276` (same predicate over the merged values, same comment, same body):

    if !merged_resource_tags.is_empty() && !merged_exclude_resource_tags.is_empty() {
        // A rule the MODEL cannot express, stated in the same envelope a modelled
        // constraint takes: the member that cannot hold, and what it must satisfy.
        return Err(UpdateRuleErrorModeled::ValidationException(
            crate::state::member_validation_failed(
                crate::state::EXCLUDE_RESOURCE_TAGS_MEMBER,
                crate::state::EXCLUDE_RESOURCE_TAGS_REQUIREMENT,
            ),
        )
        .into());
    }

The policy the crate states for exactly this situation — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/state.rs:576-582` (doc of `unlockable_rule`):

    /// resource tags is tag-level, and one carrying exclusion tags is a Region-level rule
    /// with exclusions. The invariant lives HERE, in one place, because every door the
    /// combination can arrive through is held to it — `LockRule`, which refuses the rule the
    /// request addressed, `CreateRule`, which refuses a request that would mint the same
    /// combination, and `UpdateRule`, which refuses a request that would change a
    /// lock-holding rule's selection into one — so no two of them can disagree about which
    /// rules a lock can hold on.

and the same policy for the retention bound — `src/state.rs:748-751`:

    /// Every door onto a rule's retention period goes through this: `CreateRule`, where the
    /// type is the one the request names, and `UpdateRule`, where it is the stored one (the
    /// type cannot be updated), so no sequence of calls leaves a rule holding a period the
    /// service could not retain.

Test coverage that must stay green and unchanged:
* `tests/all/retention_rule_lifecycle.rs:441-465` — `exclusion_tags_cannot_be_combined_with_resource_tags` drives CreateRule with both members non-empty and asserts HTTP 400 with `Reason = INVALID_PARAMETER_VALUE` (covers copy 1).
* `tests/all/update_rule_lock_invariant.rs:326-356` — `update_rule_refuses_every_selection_a_rule_holding_a_lock_cannot_take` pins UpdateRule's neighbouring selection refusal (`lock_holding_selection_not_allowed`) message and `Reason`, i.e. the guard immediately after copy 2.
* `tests/all/lockable_rule_invariant.rs:200-229` pins the `lockConfiguration` refusal wording built from the same shared constants (`TAG_LEVEL_REQUIREMENT`), so a hoist that reuses them cannot change the text.

Note that copy 2 (the merged-selection path of UpdateRule) has no direct test of its own — `grep -n 'ExcludeResourceTags' tests/all/update_rule_lock_invariant.rs` shows every row either sends one selection member alone or sends the other one empty — which is precisely why the two copies can drift unnoticed.

## Steps to Reproduce

1. Read services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs:214-227 — the `!resource_tags.is_empty() && !exclude_resource_tags.is_empty()` predicate plus the refusal it composes.
2. Read services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/update_rule.rs:266-276 — the same predicate over the merged values, with the same comment and the same refusal.
3. Read src/state.rs:576-582 and src/state.rs:748-751 — the crate states that every multi-door invariant (lockability, per-type retention bound) lives in one place in `crate::state` so two doors cannot disagree; this one does not.
4. Observe that only the two interpolated constants (EXCLUDE_RESOURCE_TAGS_MEMBER at src/state.rs:648, EXCLUDE_RESOURCE_TAGS_REQUIREMENT at src/state.rs:665) are shared — the rule itself is duplicated.
5. Confirm the drift risk: tests/all/retention_rule_lifecycle.rs:441 covers the CreateRule copy, while no test sends both selection members non-empty to UpdateRule.
