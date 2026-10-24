Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.update-scope-add-target-reflected networkflowmonitor.update-scope-remove-target-reflected networkflowmonitor.update-monitor-overlapping-add-remove-rejected networkflowmonitor.get-monitor-nonexistent-rejected networkflowmonitor.delete-monitor-removes-resource networkflowmonitor.create-monitor-duplicate-name-rejected networkflowmonitor.list-monitors-invalid-next-token-rejected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor.update-scope-add-target-reflected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException for cross-region add, got {code!r}"
Issue-Key: networkflowmonitor.update-scope-remove-target-reflected|run#9|AssertionError|assert code == "ValidationException", (
Issue-Key: networkflowmonitor.update-monitor-overlapping-add-remove-rejected|run#12|AssertionError|assert code == "ValidationException", f"expected ValidationException for overlapping add/remove, got {code!r}"
Issue-Key: networkflowmonitor.get-monitor-nonexistent-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: networkflowmonitor.delete-monitor-removes-resource|run#15|AssertionError|assert again_message == gone_message, f"expected message {gone_message!r}, got {again_message!r}"
Issue-Key: networkflowmonitor.create-monitor-duplicate-name-rejected|run#12|AssertionError|assert message == "Cannot create a monitor which already exists", (
Issue-Key: networkflowmonitor.list-monitors-invalid-next-token-rejected|run#5|AssertionError|assert message == "NextToken provided isn't valid.", f"expected the invalid-token message, got {message!r}"
Issue-Commit: bd3308b0c8d992d57414e8fe0564c68280e702c4

# NetworkFlowMonitor control plane: UpdateScope answers 501, UpdateMonitor skips overlap validation, and the not-found / conflict / invalid-token error messages do not follow AWS's rules (5 causes)

> _Note: this issue was identified against the code base at commit `bd3308b0c8d992d57414e8fe0564c68280e702c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateScope

## Description
Five distinct root causes in service networkflowmonitor's control plane. All are grounded in failing behavioral checks that the suite did NOT mark as already filed.

=== Cause 1 of 5: UpdateScope is a 501 stub, so none of its refusals exist ===
AWS: Smithy model services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json — UpdateScope is PATCH /scopes/{scopeId}, returns 200 with required `scopeId`, `status`, `scopeArn`, and models ValidationException (400), ConflictException (409), ResourceNotFoundException (404), ServiceQuotaExceededException (402), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500). `NotImplemented`/501 is in no error set. Recorded behavior: `resourcesToAdd` naming a target whose `region` differs from the service Region is refused with HTTP 400 `ValidationException`, message example `invalid input -> (invalid targets -> (The target region must match the service region -> (for the specified target resource)))`; `resourcesToDelete` removing the scope's only remaining target is refused with HTTP 400 `ValidationException`, message example `invalid input -> (owner [<accountId>] cannot be removed from scope.)` (the message is built from the request — the owning account id). In both cases the scope's `targets` and `SUCCEEDED` status are unchanged.
Emulator: services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/update_scope.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateScope"))` → HTTP 501, error code `NotImplemented`.
Why: a client can never add or remove scope targets, cannot reactivate a deactivated scope, and gets an unmodeled error code that no SDK error handler can classify instead of the 400 that tells it which target is invalid.
Fix: implement UpdateScope in its own handler file (src/ops/update_scope.rs) over scope state in src/state.rs — add/remove targets, enforce target-region == service-region and "a scope must keep at least one target" as modeled ValidationExceptions, leave state untouched on refusal, and delete the SCAFFOLDED marker.
Traces: networkflowmonitor.update-scope-add-target-reflected, networkflowmonitor.update-scope-remove-target-reflected.

=== Cause 2 of 5: UpdateMonitor does not refuse a resource that is in both add and remove ===
AWS: UpdateMonitor is PATCH /monitors/{monitorName} with `localResourcesToAdd`/`localResourcesToRemove`/`remoteResourcesToAdd`/`remoteResourcesToRemove` and models ValidationException (400). Recorded behavior: a request listing the same resource in an *ToAdd and its *ToRemove list is refused with HTTP 400 `ValidationException` and the monitor is unchanged.
Emulator: the call returns HTTP 501 with error code `NotImplemented` (the handler src/ops/update_monitor.rs is scaffolded), so the overlap rule is absent.
Why: a self-contradictory update is accepted-or-mis-rejected rather than refused with a classifiable 400, and a client cannot tell a bad request from an unimplemented service.
Fix: UpdateMonitor's handler and its input validation. Related: the auto-filed scaffold issue "Implement networkflowmonitor UpdateMonitor: unimplemented scaffold stub" covers merely standing the operation up; this cause additionally requires the add/remove overlap refusal, so closing that stub issue alone does not satisfy this check.
Traces: networkflowmonitor.update-monitor-overlapping-add-remove-rejected.

=== Cause 3 of 5: monitor ResourceNotFoundException message is not built from the monitor ARN ===
AWS: GetMonitor and DeleteMonitor model ResourceNotFoundException (HTTP 404). Recorded behavior: the message is built from the request and names the monitor's full ARN — `Monitor [arn:aws:networkflowmonitor:<region>:<accountId>:monitor/<monitorName>] doesn't exist`. The rule is therefore known: ARN-qualified identifier in square brackets, suffix `doesn't exist`.
Emulator: the status and code are right (404 / ResourceNotFoundException) but the message is the bare name, e.g. `monitor <monitorName> was not found` — neither ARN-qualified nor in AWS's form. This is a systematic difference in how the emulator renders this error, not one recording's wording.
Why: clients and tools that parse or log the ARN out of the not-found message (a common way to identify which resource of a batch was missing) get no ARN at all.
Fix: the not-found error construction shared by the monitor read/delete handlers (src/ops/get_monitor.rs, src/ops/delete_monitor.rs) — render the monitor ARN and AWS's message form.
Traces: networkflowmonitor.get-monitor-nonexistent-rejected, networkflowmonitor.delete-monitor-removes-resource.

=== Cause 4 of 5: CreateMonitor duplicate-name ConflictException message diverges ===
AWS: CreateMonitor models ConflictException (HTTP 409). Recorded behavior: creating a monitor whose `monitorName` already exists is refused with the fixed, request-independent text `Cannot create a monitor which already exists` (the recorded message contains no request value, so the rule is a constant for this condition), and the existing monitor's `localResources` are not overwritten.
Emulator: status and code are right (409 / ConflictException) but the message is request-interpolated instead, e.g. `monitor <monitorName> already exists`.
Why: the emulator's text is not the constant AWS produces, so a client matching on the documented conflict text does not recognise the condition.
Fix: the conflict error construction in CreateMonitor's handler (src/ops/create_monitor.rs).
Traces: networkflowmonitor.create-monitor-duplicate-name-rejected.

=== Cause 5 of 5: ListMonitors invalid-nextToken ValidationException message diverges ===
AWS: ListMonitors is GET /monitors with `nextToken` bound to the `nextToken` query string and models ValidationException (HTTP 400). Recorded behavior: a `nextToken` the service did not issue is refused with the fixed text `NextToken provided isn't valid.` (again request-independent, so the rule is a constant for this condition).
Emulator: status and code are right (400 / ValidationException) but the message is `nextToken: is not a token this service issued`.
Why: paginating clients that key off AWS's invalid-token text to restart a scan do not recognise the condition.
Fix: the pagination-token validation in ListMonitors' handler (src/ops/list_monitors.rs).
Traces: networkflowmonitor.list-monitors-invalid-next-token-rejected.

## Evidence
Suite: 4/24 passed for networkflowmonitor. In every message cause below the HTTP status and error code already match AWS; the divergence is the message rule.

Cause 1 — UpdateScope answers an unmodeled 501/NotImplemented:
  networkflowmonitor.update-scope-add-target-reflected:
    assert code == "ValidationException"
    AssertionError: expected ValidationException for cross-region add, got 'NotImplemented'
    Expected: HTTP 400 ValidationException, message example `invalid input -> (invalid targets -> (The target region must match the service region -> (for the specified target resource)))`, scope targets/status unchanged.
  networkflowmonitor.update-scope-remove-target-reflected:
    assert code == "ValidationException"
    AssertionError: expected ValidationException removing the scope's only target, got 'NotImplemented'
    Expected: HTTP 400 ValidationException, message example `invalid input -> (owner [<accountId>] cannot be removed from scope.)`.
  Checkout corroboration: services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops/update_scope.rs lines 3-6 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 180 (`Err(crate::scaffolded::Scaffolded::not_implemented("UpdateScope"))`).

Cause 2 — networkflowmonitor.update-monitor-overlapping-add-remove-rejected:
  assert code == "ValidationException"
  AssertionError: expected ValidationException for overlapping add/remove, got 'NotImplemented'
  Expected: HTTP 400 ValidationException; actual: HTTP 501 NotImplemented.

Cause 3 — monitor not-found message:
  networkflowmonitor.get-monitor-nonexistent-rejected:
    AssertionError: expected message "Monitor [arn:aws:networkflowmonitor:us-west-2:<account>:monitor/<name>] doesn't exist", got 'monitor <name> was not found'
  networkflowmonitor.delete-monitor-removes-resource (second GetMonitor after the delete):
    AssertionError: expected message "Monitor [arn:aws:networkflowmonitor:us-west-2:<account>:monitor/<name>] doesn't exist", got 'monitor <name> was not found'
  Code asserted first and matched: ResourceNotFoundException (HTTP 404).

Cause 4 — networkflowmonitor.create-monitor-duplicate-name-rejected:
  assert code == "ConflictException"  -> passed (HTTP 409)
  assert message == "Cannot create a monitor which already exists"
  AssertionError: expected the duplicate-monitor message, got 'monitor <name> already exists'

Cause 5 — networkflowmonitor.list-monitors-invalid-next-token-rejected:
  code asserted first and matched: ValidationException (HTTP 400)
  assert message == "NextToken provided isn't valid."
  AssertionError: expected the invalid-token message, got 'nextToken: is not a token this service issued'

## Steps to Reproduce
1. Boot the emulator and point a boto3 `networkflowmonitor` client at it; note the caller's account id from STS GetCallerIdentity.
2. Cause 1: CreateScope with one ACCOUNT target in the caller's account/Region and wait for SUCCEEDED. Call `update_scope(scopeId=<id>, resourcesToAdd=[{"targetIdentifier": {"targetId": {"accountId": <account>}, "targetType": "ACCOUNT"}, "region": <a different region>}])` — observe HTTP 501 `NotImplemented` instead of HTTP 400 `ValidationException`. Then call `update_scope(scopeId=<id>, resourcesToDelete=[<the scope's only target>])` — same 501, where AWS refuses with 400 `ValidationException`. In both cases GetScope must still show the original targets and SUCCEEDED.
3. Cause 2: CreateMonitor, then `update_monitor(monitorName=<name>, localResourcesToAdd=[R], localResourcesToRemove=[R])` with the SAME resource R in both lists — observe HTTP 501 `NotImplemented` instead of HTTP 400 `ValidationException`.
4. Cause 3: `get_monitor(monitorName="<never-created>")` — observe 404 `ResourceNotFoundException` with message `monitor <name> was not found` instead of `Monitor [arn:aws:networkflowmonitor:<region>:<account>:monitor/<name>] doesn't exist`. Repeat after DeleteMonitor on a monitor that did exist.
5. Cause 4: CreateMonitor with a `monitorName` that already exists — observe 409 `ConflictException` with message `monitor <name> already exists` instead of `Cannot create a monitor which already exists`.
6. Cause 5: `list_monitors(nextToken="not-a-token")` — observe 400 `ValidationException` with message `nextToken: is not a token this service issued` instead of `NextToken provided isn't valid.`
7. Re-replay: networkflowmonitor.update-scope-add-target-reflected, networkflowmonitor.update-scope-remove-target-reflected, networkflowmonitor.update-monitor-overlapping-add-remove-rejected, networkflowmonitor.get-monitor-nonexistent-rejected, networkflowmonitor.delete-monitor-removes-resource, networkflowmonitor.create-monitor-duplicate-name-rejected, networkflowmonitor.list-monitors-invalid-next-token-rejected.
