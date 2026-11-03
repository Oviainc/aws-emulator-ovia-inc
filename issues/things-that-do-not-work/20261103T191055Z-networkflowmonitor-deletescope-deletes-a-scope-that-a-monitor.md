Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.delete-scope-with-referencing-monitor-rejected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor.delete-scope-with-referencing-monitor-rejected|_poll_until#5|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Commit: f8bf972e45ad9cc3c3d155db5b98d6bbeb81e357

# networkflowmonitor: DeleteScope deletes a scope that a monitor still references instead of refusing it (no referencing-monitor guard), leaving monitors pointing at a scope ARN that no longer resolves

> _Note: this issue was identified against the code base at commit `f8bf972e45ad9cc3c3d155db5b98d6bbeb81e357`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScope

## Description
One net-new root cause in the NetworkFlowMonitor scope surface, grounded in the failing behavioral check networkflowmonitor.delete-scope-with-referencing-monitor-rejected (group 'overly-permissive'). The only open networkflowmonitor issue (20261029T205920Z-networkflowmonitor-two-causes-the-one-scope-per-account.md) covers the CreateScope singleton quota and the UpdateScope duplicate-add rule; it says nothing about DeleteScope's referential guard, so it does not cover this.

Cause 1 of 1: DeleteScope performs the delete even while a monitor still references the scope, instead of refusing the request.

AWS: DeleteScope is a client-refusable destructive operation whose modeled errors include ConflictException (errorKind client, httpError 409 — "The requested resource is in use") and ValidationException (errorKind client, httpError 400 — "Invalid request") (Smithy: DeleteScope errors = AccessDenied/Conflict/InternalServer/ResourceNotFound/ServiceQuotaExceeded/Throttling/Validation; http DELETE /scopes/{scopeId}, @idempotent). A monitor is created against a scope by ARN (Smithy: CreateMonitor.scopeArn, Arn, @required), so a live monitor is a reference to the scope. Recorded behavior: a DeleteScope naming a scope that a monitor still references is REFUSED — either 409 ConflictException with the message whose fixed text is "Scope [" ... "] is being mutated" (the variable part is the scope's own scopeArn, taken from the addressed scope's state), or 400 ValidationException with the wholly fixed message "Scope can't be deleted as there's monitors associated with it" (no variable parts). The refusal happens before any write, so the scope remains in its terminal SUCCEEDED status (not DEACTIVATING/DEACTIVATED and not removed). Once the referencing monitor is deleted, the same DeleteScope succeeds and the scope is actually removed, so a following GetScope answers ResourceNotFoundException. AWS appends a per-call " (Service: ..., Status Code: ..., Request ID: ...)" suffix to the message, which the check strips and which is NOT part of the requirement.

Emulator: there is no referential guard at all. services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/delete_scope.rs validates only the FORM of the scopeId (state::is_scope_id -> ValidationException) and then the scope's EXISTENCE (state.scopes.remove -> ResourceNotFoundException), and otherwise removes the scope record, drops its ARN from state.arn_index and purges its idempotency entries, answering the empty DeleteScopeOutput with 200. The monitor store (state.monitors, whose records hold the scope_arn they were created under — src/state.rs Monitor::scope_arn) is never consulted, so neither the modeled ConflictException nor the modeled ValidationException is reachable for this condition, and the delete always wins.

Why: a client that relies on AWS's referential refusal to protect a live monitoring configuration gets the opposite outcome. The scope disappears while monitors still name its ARN, so every such monitor is left dangling: its scopeArn resolves to nothing (the emulator even removes the ARN from arn_index), and no error ever told the caller the scope was in use. Teardown and reconciliation logic that deletes bottom-up by catching ConflictException/ValidationException from DeleteScope — exactly the pattern this check exercises, and the pattern AWS's docs require — never takes that branch, so it silently skips deleting the monitors first and ends with an inconsistent topology that cannot be repaired by re-creating the scope (a new scope gets a new ARN).

Fix: DeleteScope's handler/validation in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/delete_scope.rs. Under the same store lock that performs the removal, after the scopeId form check and the existence lookup but BEFORE the record is removed from state.scopes (and before arn_index/scope_idempotency are touched), reject when any monitor in state.monitors carries that scope's scope_arn, returning the modeled refusal with AWS's wording; the message builder belongs in src/state.rs beside the existing scope_not_found()/invalid_scope_id() helpers. The refusal must leave the scope and its indices byte-for-byte unchanged, so a following GetScope still reports SUCCEEDED, and the delete must still succeed (and still remove the scope) once no monitor references it.

Traces: networkflowmonitor.delete-scope-with-referencing-monitor-rejected. Code citations: src/ops/delete_scope.rs:150-195 (form check, then state.scopes.remove, then arn_index/scope_idempotency cleanup — no monitor lookup anywhere), src/state.rs:287 (Monitor::scope_arn, the reference that is never consulted).

## Evidence
Check networkflowmonitor.delete-scope-with-referencing-monitor-rejected (service heading: networkflowmonitor; verdict: mismatch).

Expected (AWS): with a monitor created against the scope's scopeArn and ACTIVE, DeleteScope(scopeId=<that scope>) is refused — 409 ConflictException "Scope [<scopeArn>] is being mutated" or 400 ValidationException "Scope can't be deleted as there's monitors associated with it" — and GetScope afterwards still reports status SUCCEEDED. After DeleteMonitor removes the referencing monitor, the same DeleteScope succeeds and GetScope then answers ResourceNotFoundException.

Actual (emulator): DeleteScope has no referencing-monitor guard, so the call is accepted: src/ops/delete_scope.rs:174 removes the scope with state.scopes.remove(&input.scope_id), src/ops/delete_scope.rs:188-191 removes its ARN from arn_index and purges its idempotency records, and the handler returns the memberless DeleteScopeOutput with HTTP 200. Neither ConflictException nor ValidationException can be produced for this condition anywhere in the file — the monitor store is not read. Monitors created with that scopeArn (src/state.rs:287) survive the delete and now reference a scope ARN the store no longer resolves.

Replay-run note (so the grounding is not overstated): in this run the check aborted BEFORE issuing the graded DeleteScope — it failed in its scope-status wait with "AssertionError: condition not met within 1.0s; last='IN_PROGRESS'" (check_networkflowmonitor_delete_scope_with_referencing_monitor_rejected.py:174 -> _poll_until:233). That wait collapsed to a 1.0s deadline from the check's own remaining-time arithmetic (scope_wait = max(1.0, min(1200.0, time_left - 300.0))) while polling at a 5.0s interval, so it got exactly one probe; a freshly created scope reporting IN_PROGRESS at that first probe is itself faithful (src/state.rs:56 PROVISIONING_DELAY = 500ms, materialized by Scope::settle on the next call, and a sibling check does observe SUCCEEDED). The emulator defect reported above is therefore read off the handler source in the checkout rather than off the aborted replay, and a fixer should re-replay this check id to confirm both the refusal and the positive control.

## Steps to Reproduce
1. Boot the emulator and point a networkflowmonitor client at it; note the caller's account id from sts:GetCallerIdentity.
2. Acquire the account's scope: CreateScope(targets=[{targetIdentifier: {targetId: {accountId: <account>}, targetType: 'ACCOUNT'}, region: <request region>}]) and poll GetScope until status is SUCCEEDED (the emulator settles it within ~500ms; poll with a sub-second interval).
3. CreateMonitor(monitorName=<name>, localResources=[{type: 'AWS::Region', identifier: <request region>}], scopeArn=<the scope's scopeArn>) and poll GetMonitor until monitorStatus is ACTIVE.
4. Call DeleteScope(scopeId=<that scope>) while that monitor still exists.
   Observed: HTTP 200 with an empty body; the scope is gone (a following GetScope answers ResourceNotFoundException) while the monitor still exists and still carries the deleted scope's ARN.
   Expected (AWS): 409 ConflictException "Scope [<scopeArn>] is being mutated" or 400 ValidationException "Scope can't be deleted as there's monitors associated with it", with GetScope still reporting SUCCEEDED.
5. Positive control that must keep working after the fix: DeleteMonitor(monitorName=<name>), wait until GetMonitor answers ResourceNotFoundException, then DeleteScope(scopeId=<that scope>) — this must succeed and remove the scope, so GetScope answers ResourceNotFoundException.

Or replay directly: run_behavioral_suite and re-run the check in trace_ids.
