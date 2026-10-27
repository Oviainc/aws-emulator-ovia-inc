Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b827135cf833dfec17047e08babbbec809d0cc25

# DeleteAnomalyDetector accepts a detector that is CREATING or UPDATING, and a workspace that is not ACTIVE, where every sibling delete in this crate answers ConflictException

_Filed: 2026-10-26T08:25:28Z_
_Operation: DeleteAnomalyDetector | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b827135cf833dfec17047e08babbbec809d0cc25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteAnomalyDetector` is the only resource-deleting handler in this crate that applies NO status guard. After resolving the workspace and the detector it special-cases only the already-`DELETING` record (the idempotent replay) and otherwise moves whatever it found straight to `DELETING`:

```rust
let _ = input.client_token;
if detector.status_code != crate::state::STATUS_DELETING {
    detector.status_code = crate::state::STATUS_DELETING.to_string();
```

So a detector that is still `CREATING` (the status every create leaves it in for `TRANSITION_DELAY`), or one still settling an `UPDATING` that `PutAnomalyDetector` started, is deleted with 202 — and the parent workspace's own status is never looked at at all, so a detector inside a `CREATING` workspace is deletable too.

That contradicts this crate's own rule for the same resource and for every sibling: `PutAnomalyDetector` refuses to touch a detector, or a workspace, that is not `ACTIVE` with the modelled `ConflictException`, stating the rule as a crate-wide one; `DeleteScraper`, `DeleteWorkspace`, `DeleteRuleGroupsNamespace`, `DeleteAlertManagerDefinition`, `DeleteLoggingConfiguration` and `DeleteResourcePolicy` all refuse a non-ACTIVE record (and, for the child resources, a non-ACTIVE parent workspace) the same way. `DeleteAnomalyDetector` declares `ConflictException (HTTP 409)` in its own modelled errors, so the refusal the siblings use is available to it.

Effect: the same detector can be refused 409 by `PutAnomalyDetector` and accepted 202 by `DeleteAnomalyDetector` at the same instant, and a delete racing a create/retrain silently discards the transition the earlier call left owed.

## Evidence

Handler, no status guard — `src/ops/delete_anomaly_detector.rs:214-225`:
```
        // The deletion is asynchronous (modelled 202): the detector moves to `DELETING`
        // and the clock owes its removal. A second call while that is pending starts
        // nothing new ...
        let _ = input.client_token;
        if detector.status_code != crate::state::STATUS_DELETING {
            detector.status_code = crate::state::STATUS_DELETING.to_string();
            detector.transition_due = Some(now + crate::state::TRANSITION_DELAY);
            detector.modified_at = now;
        }
```
(the workspace is only resolved, never status-checked: `src/ops/delete_anomaly_detector.rs:189-198`).

Sibling rule for the SAME resource — `src/ops/put_anomaly_detector.rs:282-309`:
```
            // A detector that is not usable yet — still being created, still settling a
            // previous retraining, or being deleted — cannot take a new specification, and
            // neither can one inside a workspace that is not usable: ... `ACTIVE` is
            // the one usable status, as it is for every other resource of this service.
            if workspace_status != crate::state::STATUS_ACTIVE {
                return Err(conflict(...));
            }
            if detector.status_code != crate::state::STATUS_ACTIVE {
                return Err(conflict(
                    format!(
                        "The anomaly detector is {}; updates are allowed only when status is {}",
```

Sibling delete — `src/ops/delete_scraper.rs:191-204`:
```
        if !replays_pending_delete && scraper.status_code != crate::state::STATUS_ACTIVE {
            return Err(DeleteScraperErrorModeled::ConflictException(
```
and `src/ops/delete_rule_groups_namespace.rs:240-270` refuses both a non-ACTIVE namespace and a non-ACTIVE parent workspace.

The refusal is modelled for this operation: smithy_query(operation="DeleteAnomalyDetector") lists `{"shape": "ConflictException", "errorKind": "client", "httpError": 409}`.

Test coverage: no test pins the current leniency. `tests/all/anomaly_detector_lifecycle.rs:575-620` (`delete_starts_a_removal_the_clock_then_completes`) calls `wait_for_due_transition();` at line 598 BEFORE deleting, i.e. it deletes an ACTIVE detector, and `tests/all/anomaly_detector_lifecycle.rs:724` only covers the 404 path — so adding the sibling guard leaves the existing tests unchanged.

## Steps to Reproduce

1. CreateWorkspace and wait for it to reach ACTIVE.
2. CreateAnomalyDetector in that workspace; the response reports status CREATING (`src/ops/create_anomaly_detector.rs:324`).
3. Immediately (within crate::state::TRANSITION_DELAY) call PutAnomalyDetector on that detector id: refused 409 ConflictException, "The anomaly detector is CREATING; updates are allowed only when status is ACTIVE".
4. Immediately call DeleteAnomalyDetector on the SAME detector id: answered 202 and the detector moves to DELETING, then disappears — the crate accepts a transition out of CREATING that it refuses for every other resource and for the same detector's update path.
5. Variant: create a workspace and, while it is still CREATING, delete a detector inside it — accepted, where DeleteRuleGroupsNamespace/DeleteLoggingConfiguration refuse 409 for a non-ACTIVE parent workspace.
