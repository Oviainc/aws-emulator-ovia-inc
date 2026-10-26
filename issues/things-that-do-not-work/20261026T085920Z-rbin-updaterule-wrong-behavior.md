Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f4da987cef45329e7b9910a2761caf42f0af6ecf

# UpdateRule can turn a lock-configured rule into a tag-level / exclusion-tag rule, a combination CreateRule and LockRule both refuse

_Filed: 2026-10-26T08:59:20Z_
_Operation: UpdateRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f4da987cef45329e7b9910a2761caf42f0af6ecf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate centralises the "which rules can hold a lock" invariant in `crate::state::unlockable_rule` and holds every door that can produce a locked rule to it — CreateRule refuses a `LockConfiguration` together with `ResourceTags`/`ExcludeResourceTags` with a 400, and LockRule refuses the same rule with a 409. UpdateRule is the third door that can change exactly the two members that function reads (`ResourceTags`, `ExcludeResourceTags`), and it never consults it, even though `Rule::unlockable()` exists for precisely that purpose and is called by LockRule.

So a rule that was created with a `LockConfiguration` (hence Region-level, no exclusion tags), then unlocked and whose unlock delay has run out (`lock_state == unlocked`, so `is_lock_protected` no longer blocks a modify), can be updated to carry `ResourceTags` (or `ExcludeResourceTags`). The stored record then holds a `LockConfiguration` and a `LockState` on a tag-level rule — a record CreateRule refuses to mint with HTTP 400 ValidationException and LockRule refuses to produce with HTTP 409 ConflictException. GetRule afterwards reports that impossible combination, and LockRule on the same rule now answers 409 "because it is a tag-level rule", so the rule advertises a lock configuration it can never return to.

The same handler already re-validates the OTHER model-inexpressible combination against the merged state (resource tags + exclusion tags, update_rule.rs:248), which makes the omission of the lock-configuration half an inconsistency inside one handler as well as against its two sibling handlers.

## Evidence

`src/state.rs:465-474` (doc of `UnlockableRule`): "The invariant lives HERE, in one place, because every door a lock can arrive through is held to it — `LockRule`, which refuses the rule the request addressed, and `CreateRule`, which refuses a request that would mint the same combination — so the two cannot disagree about which rules a lock can hold on."

`src/ops/create_rule.rs:228-242`: "Recycle Bin holds a lock only on a Region-level rule that carries no exclusion tags ... the same invariant `LockRule` enforces on an existing rule, so no door leaves behind a locked rule the service could never have produced." followed by `let unlockable = input.lock_configuration.as_ref().and_then(|_| crate::state::unlockable_rule(&resource_tags, &exclude_resource_tags)); if let Some(blocker) = unlockable { return Err(... lock_configuration_not_allowed(blocker)) }`.

`src/ops/lock_rule.rs:203-212`: `if let Some(blocker) = rule.unlockable() { return Err(LockRuleErrorModeled::ConflictException(...)) }`, with `src/state.rs:227-231` — `/// Whether the rule's own resource selection makes it one Recycle Bin cannot lock, as [unlockable_rule] decides.  pub fn unlockable(&self) -> Option<UnlockableRule>`.

`src/ops/update_rule.rs:248-258` re-checks only one combination — `if !merged_resource_tags.is_empty() && !merged_exclude_resource_tags.is_empty() { ... EXCLUDE_RESOURCE_TAGS_REQUIREMENT ... }` — and `src/ops/update_rule.rs:284-285` then writes the new selection with no lock check at all: `rule.resource_tags = merged_resource_tags; rule.exclude_resource_tags = merged_exclude_resource_tags;`. Neither `unlockable` nor `unlockable_rule` nor `lock_configuration` appears anywhere in update_rule.rs's handler body.

Model text the invariant comes from (quoted by the crate at src/state.rs:468-469): "You can't lock tag-level retention rules, or Region-level retention rules that have exclusion tags".

Test coverage: `tests/all/lockable_rule_invariant.rs:3-13` states the rule for "EITHER door" (CreateRule 400, LockRule 409) and its only UpdateRule call (line 311) updates `Description` alone, so no test exercises an update that changes a lock-configured rule's resource selection.

## Steps to Reproduce

1. POST /rules with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"LockConfiguration":{"UnlockDelay":{"UnlockDelayValue":7,"UnlockDelayUnit":"DAYS"}}} -> 201, LockState=locked.
2. PATCH /rules/{Identifier}/unlock -> 200, LockState=pending_unlock, LockEndTime = now + 7 days.
3. Let the unlock delay elapse (GetRule then reports LockState=unlocked, via `rule.settle(now)` in get_rule.rs:193), so the rule is no longer lock-protected.
4. PATCH /rules/{Identifier} with {"ResourceTags":[{"ResourceTagKey":"team","ResourceTagValue":"ops"}]} -> actual 200, the rule is now tag-level while still holding its LockConfiguration/LockState. Expected: refused, as CreateRule refuses the same combination with 400 ValidationException ("Member must not be specified for a tag-level retention rule").
5. GET /rules/{Identifier} -> body carries both ResourceTags and LockConfiguration, a record the service has no other way to produce.
6. PATCH /rules/{Identifier}/lock -> 409 ConflictException "because it is a tag-level rule", proving the stored rule advertises a lock state it can never reach again.
7. Repeat step 4 with {"ExcludeResourceTags":[{"ResourceTagKey":"skip","ResourceTagValue":"yes"}]} for the second half of the same invariant.
