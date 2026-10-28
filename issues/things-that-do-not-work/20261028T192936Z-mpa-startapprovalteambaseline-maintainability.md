Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: e8dc968dccb3edde6e1cac399927b1a6964cab15

# The session-ceiling rule (predicate plus its sentence) is written out three times, once per session-starting handler

_Filed: 2026-10-28T19:29:36Z_
_Operation: StartApprovalTeamBaseline | Class: maintainability_

> _Note: this issue was identified against the code base at commit `e8dc968dccb3edde6e1cac399927b1a6964cab15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`if state.sessions.len() >= crate::state::MAX_SESSIONS` and the sentence it answers with ("The account has reached its quota of {} approval sessions.") are copied into all three handlers that start a session, so the three copies can drift in predicate (`>=` vs `>`), in the count they compare, and in wording.

This crate already states the pattern for exactly this situation one level up: the per-resource tag ceiling used to be written out once per create and now lives once in `crate::state` (`tags_past_resource_ceiling`), with each create lifting the single refusal into the error ITS model declares. The session ceiling is the same shape of rule — one predicate, one sentence, three operations whose declared errors differ — and is the remaining duplicate. Moving the predicate and the sentence into `crate::state` (beside `MAX_SESSIONS`, whose own doc already owns the reason for the bound) and having each handler wrap the result in its own declared error leaves every response byte-identical and every existing test passing.

## Evidence

Copy 1 — ops/update_approval_team.rs:344-356:
```
// The store is bounded, so a caller looping on this operation cannot grow
// this process's memory without end (`crate::state::MAX_SESSIONS`). ...
if state.sessions.len() >= crate::state::MAX_SESSIONS {
    return Err(UpdateApprovalTeamErrorModeled::ServiceQuotaExceededException(
        ServiceQuotaExceededException { message: format!(
            "The account has reached its quota of {} approval sessions.",
            crate::state::MAX_SESSIONS
```
Copy 2 — ops/start_approval_team_baseline.rs:261-272:
```
if state.sessions.len() >= crate::state::MAX_SESSIONS {
    return Err(StartApprovalTeamBaselineErrorModeled::ValidationException(
        ValidationException { message: format!(
            "The account has reached its quota of {} approval sessions.",
            crate::state::MAX_SESSIONS
```
Copy 3 — ops/start_active_approval_team_deletion.rs:247-262:
```
if state.sessions.len() >= crate::state::MAX_SESSIONS {
    return Err(StartActiveApprovalTeamDeletionErrorModeled::ValidationException(
        ValidationException { message: format!(
            "The account has reached its quota of {} approval \
             sessions.",
            crate::state::MAX_SESSIONS
```
The in-crate convention the fix follows — state.rs:815-824 `pub fn tags_past_resource_ceiling(...) -> Option<ValidationException>`, cited at ops/create_approval_team.rs:293-294: "The rule itself lives once, beside the other tag rules in `crate::state`." and at ops/create_identity_source.rs:302.
state.rs:1903-1906 already anticipates the per-operation error: "Each caller keeps its own refusals, which the model fixes per operation: the store's session ceiling ([`MAX_SESSIONS`]) is reported with the error the calling operation declares for it" — the ERROR is per operation, the predicate and the sentence need not be.

Existing tests that cover all three copies and must stay unchanged: tests/all/baseline_session_minting.rs:192-234 (fills the store to `state::MAX_SESSIONS`, then asserts the refusal) and tests/all/refused_write_mints_nothing.rs:165-247 ("Fill the store to its session ceiling (`state::MAX_SESSIONS`) first").

## Steps to Reproduce

1. grep -n 'MAX_SESSIONS' services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops — three handlers each carry the same `state.sessions.len() >= crate::state::MAX_SESSIONS` predicate and the same quota sentence.
2. Compare with state.rs:815-824, where the sibling per-resource tag ceiling is implemented once and lifted into each operation's own declared error.
3. Change the ceiling or the sentence in one handler and observe that nothing keeps the other two in step.
