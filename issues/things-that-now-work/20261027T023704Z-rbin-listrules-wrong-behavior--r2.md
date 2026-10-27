Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 70d60110d97a6c21bc790d7df8d6a76c7ca8ef4a

# ListRules reports (and filters on) a stale LockState: it never settles the lock transition the clock has already made due

_Filed: 2026-10-27T02:37:04Z_
_Operation: ListRules | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `70d60110d97a6c21bc790d7df8d6a76c7ca8ef4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other lock-aware handler in this crate materialises the lock transition the clock already owes a rule before reading or reporting its lock state — `GetRule`, `LockRule`, `UnlockRule` and `UpdateRule` all call `Rule::settle(now)` / `settle_lock_state(now)` first. `ListRules` does not: it takes a read-only guard (`let store = state.lock();`) and reports `rule.to_summary()`, whose `LockState` is the raw stored token, and it also *filters* on that raw token.

Consequence, observable by a client: after `UnlockRule` the rule is stored as `pending_unlock` with a `lock_end_time`. Once that instant passes, `GetRule` answers `LockState: "unlocked"` (it settles), while `ListRules` still answers `LockState: "pending_unlock"` for the same rule — two reads of the same record disagree. Worse, the `LockState` filter keys on the same unsettled value, so `ListRules` with `{"LockState":"unlocked"}` omits a rule that is in fact unlocked, and `{"LockState":"pending_unlock"}` returns one whose unlock delay has expired, contradicting the member's own documentation.

The fix is the one the sibling handlers already use: settle each rule (or its lock state) before it is matched against the `LockState` filter and before `to_summary()` renders it.

## Evidence

The rule the crate states for itself, in `src/state.rs:366-372`:

    /// Materialise the lock transition the CLOCK has already made due: a
    /// `pending_unlock` rule whose unlock delay has expired is `unlocked`, and it
    /// no longer reports a lock end time ...
    pub fn settle_lock_state(&mut self, now: SystemTime) {

The sibling reader honours it — `src/ops/get_rule.rs:191-195`:

    // A transition the clock has already made due — `pending` -> `available`, and an
    // unlock delay that has run out — was owed at this instant whether or not
    // anybody called, so report the state the clock reached rather than the one the
    // last writer left behind.
    rule.settle(now);

Same call in `src/ops/lock_rule.rs:198` (`rule.settle(now);`), `src/ops/unlock_rule.rs:198` and `:224` (`rule.settle_lock_state(now);`), and `src/ops/update_rule.rs:228` (`rule.settle(now);`).

`ListRules` skips it — `src/ops/list_rules.rs:202-232` (no `settle`/`now` anywhere in the file; the guard is not even mutable):

    let store = state.lock();
    ...
    for (identifier, rule) in store.page_from(&region, resume_after.as_deref()) {
        ...
        if input
            .lock_state
            .as_deref()
            .is_some_and(|wanted| rule.lock_state.as_deref() != Some(wanted))
        {
            continue;
        }
        ...
        rules.push(rule.to_summary());

and `to_summary` hands the raw token straight out — `src/state.rs:298-305`:

            lock_state: self.lock_state.clone(),

The model's own sentence for the filter (smithy_query doc, `ListRulesRequest$LockState`): "The lock state of the retention rules to list. Only retention rules with the specified lock state are returned." And `RuleSummary$LockState` is modelled, so the summary does report it.

That the expiry is observable in-process is pinned by an existing test — `tests/all/update_rule_lock_invariant.rs:106-123`:

    /// The delay is at least 7 days, so it cannot elapse inside a test: the instant the
    /// transition falls due is moved into the past instead, and the next request settles the
    /// rule to `unlocked` by itself ...
    fn end_unlock_delay(fixture: &Fixture, case: &str, identifier: &str) {
        ... rule.lock_end_time = Some(past);

followed by `tests/all/update_rule_lock_invariant.rs:187-191` asserting `GetRule` then reports `Some("unlocked")`.

Test coverage of the gap: none. `grep -n 'lock_state|LockState|pending_unlock' tests/all/list_rules_page_order.rs` → "No matches", and the only `ListRules` call naming a `LockState` is `tests/all/retention_rule_lifecycle.rs:505` (`{ "ResourceType": "EC2_IMAGE", "LockState": "locked" }`), which asserts only that the page is empty. No test lists a rule whose unlock delay has expired.

## Steps to Reproduce

1. POST /rules signed for eu-west-2 with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"LockConfiguration":{"UnlockDelay":{"UnlockDelayValue":7,"UnlockDelayUnit":"DAYS"}}} — 201, LockState=locked; note Identifier.
2. PATCH /rules/{Identifier}/unlock — 200, LockState=pending_unlock, LockEndTime 7 days out.
3. Let the unlock delay elapse (or, as tests/all/update_rule_lock_invariant.rs:111 does, move the stored lock_end_time into the past).
4. GET /rules/{Identifier} — 200 with LockState="unlocked" (GetRule settles).
5. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT"} — expected the summary for that rule to report LockState="unlocked", matching GetRule; actual it reports LockState="pending_unlock".
6. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","LockState":"unlocked"} — expected the rule to be returned; actual Rules is empty. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","LockState":"pending_unlock"} — expected empty; actual the rule is returned.
