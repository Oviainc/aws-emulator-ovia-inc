Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d034a3e938ef16cd07dd30892ca1ec04abab6413

# UnlockRule's "no delay" path stores pending_unlock with no LockEndTime, which this crate's own readers treat as a lock that never expires

_Filed: 2026-10-29T01:06:44Z_
_Operation: UnlockRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d034a3e938ef16cd07dd30892ca1ec04abab6413`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UnlockRule` sets the rule's lock state to `pending_unlock` and stores whatever `crate::state::unlock_delay_end` answered as the lock end time, including `None`:

```rust
let lock_end_time = crate::state::unlock_delay_end(now, rule.lock_configuration.as_ref());
rule.lock_state = Some(crate::state::LOCK_STATE_PENDING_UNLOCK.to_string());
rule.lock_end_time = lock_end_time;
rule.settle_lock_state(now);
```

`unlock_delay_end` answers `None` on three paths: the `?` on an absent lock configuration, its `_` arm for an `UnlockDelayUnit` other than `DAYS` (whose own comment says that arm "serves no delay"), and a `checked_add` that overflows.

But `None` is NOT read as "no delay" anywhere else in the module — it is read as "no end", i.e. a delay that never finishes:

* `Rule::is_lock_protected` maps `pending_unlock` + `lock_end_time == None` to `true` (`is_none_or`), so the rule is lock-protected forever;
* `Rule::settle_lock_state` only releases a `pending_unlock` rule when `lock_end_time.is_some_and(|end| now >= end)`, so an absent end time never settles and the clock can never make the release due.

The resulting record is stuck: `UpdateRule` and `DeleteRule` answer 409 forever (both gate on `is_lock_protected`), `UnlockRule` answers 409 `NotLocked` (the state is not `locked`), and `GetRule` reports `pending_unlock` with no `LockEndTime` — which the model says is "Only returned for retention rules that have been unlocked and that are still within the unlock delay period", so the rule claims to be inside a delay period it will never leave. The rule can never be deleted.

Two statements the crate makes about this are broken by it:

1. the `_` arm's own comment says it "serves no delay", while the value it returns serves an UNBOUNDED one;
2. the sibling clock helper one page earlier, `status_transition_end`, states the opposite policy for exactly the same overflow class and fails OPEN ("a clock far enough in the future to overflow must leave the rule immediately settled"), and `UnlockRule`'s own handler comment promises the rule "leaves it when the delay its lock configuration carries has run, which the clock — not a later call — makes due".

These `None` arms are defensive today (the enum and the 7..=30 range are enforced by the gate before the handler runs), so the path is not reachable from a well-formed wire request; the defect is that the defensive answer is the most damaging one available — an undeletable rule — where the crate's own stated policy for the same class of impossibility is to settle immediately.

## Evidence

The handler stores the `Option` verbatim and promises the clock will release it — `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/unlock_rule.rs:214-224`:
```
        // "After a retention rule is unlocked, it can be modified or deleted only after
        // the unlock delay period expires": the rule enters `pending_unlock` now and
        // leaves it when the delay its lock configuration carries has run, which the
        // clock — not a later call — makes due.
        let lock_end_time = crate::state::unlock_delay_end(now, rule.lock_configuration.as_ref());
        rule.lock_state = Some(crate::state::LOCK_STATE_PENDING_UNLOCK.to_string());
        rule.lock_end_time = lock_end_time;
        // A delay that is already over at the instant it starts is over for every later
        // reader too, so settle it here rather than reporting a state the clock has
        // already left.
        rule.settle_lock_state(now);
```

The producer, whose `_` arm claims to serve NO delay — `src/state.rs:490-506`:
```
pub fn unlock_delay_end(
    now: SystemTime,
    lock_configuration: Option<&LockConfiguration>,
) -> Option<SystemTime> {
    let delay = &lock_configuration?.unlock_delay;
    ...
    let seconds = match delay.unlock_delay_unit.as_str() {
        "DAYS" => value.saturating_mul(SECONDS_PER_DAY),
        // `UnlockDelayUnit` models only `DAYS`, and the boundary validator refuses an
        // unmodelled value before a handler runs, so this arm serves no delay rather
        // than guessing a scale for a unit the model does not define.
        _ => return None,
    };
    now.checked_add(Duration::from_secs(seconds))
}
```

The readers that turn that `None` into a permanent lock — `src/state.rs:376-382`:
```
    pub fn is_lock_protected(&self, now: SystemTime) -> bool {
        match self.lock_state.as_deref() {
            Some(LOCK_STATE_LOCKED) => true,
            Some(LOCK_STATE_PENDING_UNLOCK) => self.lock_end_time.is_none_or(|end| now < end),
            _ => false,
        }
    }
```
and `src/state.rs:428-435`:
```
    pub fn settle_lock_state(&mut self, now: SystemTime) {
        if self.lock_state.as_deref() == Some(LOCK_STATE_PENDING_UNLOCK)
            && self.lock_end_time.is_some_and(|end| now >= end)
        {
            self.lock_state = Some(LOCK_STATE_UNLOCKED.to_string());
            self.lock_end_time = None;
        }
    }
```
which are the gates `UpdateRule` (`src/ops/update_rule.rs:232`: `if rule.is_lock_protected(now) {` → 409 `Modify`/`Locked`) and `DeleteRule` (`src/ops/delete_rule.rs:161`: `if rule.is_lock_protected(now) {` → 409 `Delete`/`Locked`) both decide on.

The sibling clock helper in the same module states the opposite overflow policy — `src/state.rs:301-307`:
```
/// The instant a rule that entered `pending` at `now` becomes `available`.
///
/// `checked_add` rather than `+`: a clock far enough in the future to overflow must
/// leave the rule immediately settled, not panic in a handler.
pub fn status_transition_end(now: SystemTime) -> SystemTime {
    now.checked_add(STATUS_TRANSITION).unwrap_or(now)
}
```

What the model says the absent member means — `smithy_query(query="doc", shape="GetRuleResponse", member="LockEndTime")` / `services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/update_rule.rs:85-87`: "The date and time at which the unlock delay is set to expire. Only returned for retention rules that have been unlocked and that are still within the unlock delay period."

Test coverage: `tests/all/rule_lock_transitions.rs:29-36,97-120` only exercises delays the model admits (`UNLOCK_DELAY_MIN = 7`, `UNLOCK_DELAY_MAX = 30`, `DAYS`) and asserts `LockEndTime` is one delay away (`lock_end_seconds`), so no test covers a `pending_unlock` rule whose `lock_end_time` is absent; nothing pins the current fail-closed choice.

## Steps to Reproduce

1. Create a Region-level rule (POST /rules with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"}}) and lock it (PATCH /rules/{Identifier}/lock with {"LockConfiguration":{"UnlockDelay":{"UnlockDelayValue":7,"UnlockDelayUnit":"DAYS"}}}) — HTTP 200, LockState=locked.
2. PATCH /rules/{Identifier}/unlock — HTTP 200, LockState=pending_unlock, LockEndTime = now + 7 days; GetRule reports both, and UpdateRule/DeleteRule are refused 409 while the delay runs. This is the healthy path.
3. Now take any path on which `crate::state::unlock_delay_end` answers `None` (src/state.rs:494 `lock_configuration?`, src/state.rs:503 the `_` arm for a unit other than DAYS, src/state.rs:505 a `checked_add` that overflows): src/ops/unlock_rule.rs:220 stores `rule.lock_end_time = None` while src/ops/unlock_rule.rs:219 has already set `lock_state = pending_unlock`.
4. src/ops/unlock_rule.rs:224 then calls `settle_lock_state`, which cannot fire because src/state.rs:430 requires `lock_end_time.is_some_and(..)` — the rule stays `pending_unlock` with no end time, and no later call can settle it either.
5. Observe the stuck record: GET /rules/{Identifier} reports LockState=pending_unlock with no LockEndTime (a member the model says is "only returned ... still within the unlock delay period"); PATCH /rules/{Identifier} and DELETE /rules/{Identifier} answer 409 forever (src/ops/update_rule.rs:232, src/ops/delete_rule.rs:161 both read `is_lock_protected`, which maps an absent end time to `true` at src/state.rs:379); PATCH /rules/{Identifier}/unlock answers 409 `not locked`. The rule can never be modified or deleted.
6. Compare with src/state.rs:301-307, where the same module states that an overflowing clock "must leave the rule immediately settled" — the two clock helpers disagree about which way to fail, and the `_` arm's own comment at src/state.rs:501 says it "serves no delay" while the value it returns serves an unbounded one.
