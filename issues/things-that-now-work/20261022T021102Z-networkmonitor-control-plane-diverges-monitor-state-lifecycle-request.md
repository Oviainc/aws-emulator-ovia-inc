Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmonitor.create-monitor-initial-state-pending networkmonitor.list-monitors-state-filter networkmonitor.create-monitor-invalid-name-pattern-rejected networkmonitor.monitor-name-length-boundary-rejected networkmonitor.monitor-aggregation-period-below-minimum-rejected networkmonitor.get-monitor-not-found networkmonitor.delete-monitor-not-found networkmonitor.delete-monitor-removes-from-list-monitors networkmonitor.create-monitor-duplicate-name-conflict-rejected networkmonitor.probe-invalid-source-arn-rejected networkmonitor.probe-id-pattern-validation-error networkmonitor.update-monitor-invalid-aggregation-period-rejected
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor.create-monitor-initial-state-pending|run#6|AssertionError|assert resp["state"] == "INACTIVE", f"expected INACTIVE for a zero-probe monitor, got {resp['state']!r}"
Issue-Key: networkmonitor.list-monitors-state-filter|_poll_until#5|Timeout|time.sleep(interval_s)
Issue-Key: networkmonitor.create-monitor-invalid-name-pattern-rejected|run#5|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException, got {err['Code']!r}"
Issue-Key: networkmonitor.monitor-name-length-boundary-rejected|run#4|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException, got {err['Code']!r}"
Issue-Key: networkmonitor.monitor-aggregation-period-below-minimum-rejected|run#5|AssertionError|assert err["Code"] == "BadRequestException", f"expected BadRequestException, got {err['Code']!r}"
Issue-Key: networkmonitor.get-monitor-not-found|run#11|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: networkmonitor.delete-monitor-not-found|run#10|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: networkmonitor.delete-monitor-removes-from-list-monitors|run#9|AssertionError|assert final["Message"] == expected, f"expected message {expected!r}, got {final['Message']!r}"
Issue-Key: networkmonitor.create-monitor-duplicate-name-conflict-rejected|run#9|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: networkmonitor.probe-invalid-source-arn-rejected|_error#5|AssertionError|raise AssertionError(f"expected CreateProbe to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: networkmonitor.probe-id-pattern-validation-error|run#7|AssertionError|assert positive["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {positive['Code']!r}"
Issue-Key: networkmonitor.update-monitor-invalid-aggregation-period-rejected|run#6|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: f526a401d350ce8b7b2e97003470fcfd33d10d7f

# NetworkMonitor control plane diverges: monitor state lifecycle, request-body rejection class, not-found/conflict message rules, unvalidated CreateProbe sourceArn, and NotImplemented negative paths (7 root causes)

> _Note: this issue was identified against the code base at commit `f526a401d350ce8b7b2e97003470fcfd33d10d7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMonitor

## Description
Seven distinct root causes found by replaying the behavioral suite against the emulator's NetworkMonitor service (service-com-amazonaws-networkmonitor-networkmonitor). All are in one service, so they are bundled here; each cause is independent and must be closed separately. Smithy model consulted: services/service-com-amazonaws-networkmonitor-networkmonitor/smithy-model/com-amazonaws-networkmonitor-networkmonitor.json.

Cause 1 of 7 — CreateMonitor with no probes reports state PENDING and never leaves it. Failing checks: networkmonitor.create-monitor-initial-state-pending, networkmonitor.list-monitors-state-filter.
AWS: MonitorState enum is [PENDING, ACTIVE, INACTIVE, ERROR, DELETING] (Smithy: MonitorState; CreateMonitor.state and GetMonitor.state both @required). Recorded behavior: a monitor created with zero probes comes back synchronously in state INACTIVE — there is nothing to activate without a probe — and GetMonitor immediately reports the same value; a monitor never stays in PENDING indefinitely.
Emulator: CreateMonitor returns state 'PENDING' for a zero-probe monitor, and GetMonitor keeps returning PENDING: the state-filter check polls GetMonitor for any non-PENDING state and never gets one (the check is killed at its 60s harness timeout while polling).
Why: clients that wait for a monitor to settle (a waiter, or ListMonitors(state=...) on the settled state) hang forever against the emulator, and code branching on INACTIVE for an unprobed monitor takes the wrong branch.
Fix: the CreateMonitor handler / monitor state model — initial state for a probe-free monitor, plus whatever transition the stored monitor needs so it does not remain PENDING.
Traces: networkmonitor.create-monitor-initial-state-pending (assert resp['state'] == 'INACTIVE', got 'PENDING'), networkmonitor.list-monitors-state-filter (timed out polling for state != PENDING).

Cause 2 of 7 — request-body constraint violations on CreateMonitor are answered with ValidationException instead of BadRequestException. Failing checks: networkmonitor.create-monitor-invalid-name-pattern-rejected, networkmonitor.monitor-name-length-boundary-rejected, networkmonitor.monitor-aggregation-period-below-minimum-rejected.
AWS: recorded behavior only (the error shape is not in the model — Smithy models only AccessDenied/Conflict/InternalServer/ServiceQuotaExceeded/Throttling/ValidationException for CreateMonitor; the constraints violated are monitorName @length 1..200 and @pattern ^[a-zA-Z0-9_-]+$, and aggregationPeriod @range min 30). The recordings show real NetworkMonitor rejecting a malformed POST /monitors body — a monitorName with characters outside the pattern, a monitorName of 201 chars, and aggregationPeriod=10 — at the frontend with error code BadRequestException (message example: 'Invalid request body'), i.e. before the request reaches the application code that would raise the modeled ValidationException.
Emulator: all three rejections come back with error code 'ValidationException'.
Why: a client that distinguishes a front-door body rejection (BadRequestException) from an application-level validation failure (ValidationException) — for retry decisions or error mapping — classifies these wrongly; error handlers keyed on BadRequestException never fire.
Fix: CreateMonitor input validation / the service's request-body validation layer, which must raise the frontend BadRequestException class for body-constraint violations. Note the same over-length name placed in a URL path (GetMonitor /monitors/{monitorName}) is correctly a ValidationException, so the two paths must stay distinct.
Traces: networkmonitor.create-monitor-invalid-name-pattern-rejected, networkmonitor.monitor-name-length-boundary-rejected, networkmonitor.monitor-aggregation-period-below-minimum-rejected (each: expected BadRequestException, got 'ValidationException').

Cause 3 of 7 — monitor ResourceNotFoundException message is not built from account + monitorName. Failing checks: networkmonitor.get-monitor-not-found, networkmonitor.delete-monitor-not-found, networkmonitor.delete-monitor-removes-from-list-monitors.
AWS: ResourceNotFoundException, HTTP 404 (Smithy: ResourceNotFoundException @error client, httpError 404, bound to GetMonitor/DeleteMonitor/UpdateMonitor). The message is built from the request and the caller identity by a known rule: 'No existing monitor found for account <accountId> and monitorName <monitorName>.'
Emulator: the error code is right but the message is the emulator's own template, e.g. 'monitor <name> does not exist' — the account id is absent and the sentence shape differs.
Why: clients and operators that parse or log the not-found message to recover which account/monitor was missing (and tests asserting the documented text) see a different contract; the same wrong template is returned from GetMonitor, DeleteMonitor and UpdateMonitor.
Fix: the shared not-found error construction for monitor lookups used by the GetMonitor/DeleteMonitor/UpdateMonitor handlers.
Traces: networkmonitor.get-monitor-not-found, networkmonitor.delete-monitor-not-found, networkmonitor.delete-monitor-removes-from-list-monitors (all: expected 'No existing monitor found for account 000000000000 and monitorName <name>.', got 'monitor <name> does not exist').

Cause 4 of 7 — duplicate CreateMonitor ConflictException message is not built from account + monitorName. Failing check: networkmonitor.create-monitor-duplicate-name-conflict-rejected.
AWS: ConflictException, HTTP 409 (Smithy: ConflictException @error client httpError 409, bound to CreateMonitor; doc 'This operation attempted to create a resource that already exists'). Message rule, built from the request and caller identity: 'Existing monitor found for account <accountId> and monitorName <monitorName>.'
Emulator: code ConflictException is correct, but the message is 'monitor <name> already exists (<monitorArn>)' — it omits the account and leaks the generated ARN into the message.
Why: the conflict is reported with a different contract than clients parse, and embedding the generated ARN makes the message environment-dependent.
Fix: the CreateMonitor handler's duplicate-name conflict error construction.
Traces: networkmonitor.create-monitor-duplicate-name-conflict-rejected.

Cause 5 of 7 — CreateProbe accepts a sourceArn that is not a VPC subnet. Failing check: networkmonitor.probe-invalid-source-arn-rejected.
AWS: ValidationException, HTTP 400 (Smithy: CreateProbe errors include ValidationException httpError 400; ProbeInput.sourceArn is an Arn with @pattern ^arn:.*$ only, so the resource type is checked by the service, not the schema). Recorded behavior: a well-formed ARN in the caller's own account that names a non-subnet resource (e.g. arn:aws:sns:<region>:<account>:not-a-subnet-topic) is refused with ValidationException and no probe is attached; message example 'The specified subnet not-a-subnet-topic does not exist or could not be found.'
Emulator: CreateProbe returns HTTP 200 and creates the probe, echoing the SNS ARN as sourceArn with state PENDING, so GetMonitor then lists a probe that AWS would never have created.
Why: this is overly permissive — code that relies on the service to validate subnet references silently builds monitors with nonsense sources, and the divergence is invisible until the probe is read back.
Fix: CreateProbe validation — resolve probe.sourceArn and require it to name an existing VPC subnet in the caller's account before persisting the probe.
Traces: networkmonitor.probe-invalid-source-arn-rejected (expected a rejection, call SUCCEEDED returning probeId probe-… with sourceArn arn:aws:sns:…:not-a-subnet-topic).

Cause 6 of 7 — GetProbe answers NotImplemented instead of the operation's modeled errors. Failing check: networkmonitor.probe-id-pattern-validation-error.
AWS: GetProbe GET /monitors/{monitorName}/probes/{probeId} models ResourceNotFoundException (404) and ValidationException (400) (Smithy: GetProbe errors; probeId @pattern ^probe-[a-z0-9A-Z-]{21,64}$). A well-formed but nonexistent probeId must get ResourceNotFoundException (message example 'No existing probe <probeId> found for account <accountId> and monitorName <monitorName>.'), and a probeId failing the pattern must get ValidationException.
Emulator: GetProbe answers error code 'NotImplemented' ('operation GetProbe is scaffolded but not implemented in this emulator'), so neither negative path is reachable.
Why: a client cannot distinguish a missing probe from a malformed id — both surface as a non-modeled error no SDK error handler matches — and every probe read fails.
Fix: implement the GetProbe handler, including its probeId pattern validation and its not-found path.
Traces: networkmonitor.probe-id-pattern-validation-error (expected ResourceNotFoundException, got 'NotImplemented').

Cause 7 of 7 — UpdateMonitor answers NotImplemented instead of rejecting an out-of-range aggregationPeriod. Failing check: networkmonitor.update-monitor-invalid-aggregation-period-rejected.
AWS: UpdateMonitor PATCH /monitors/{monitorName} models ValidationException (400) (Smithy: UpdateMonitor errors; aggregationPeriod @required, @range min 30). Recorded behavior: only 30 and 60 are accepted; 45 is refused with ValidationException (message example 'The aggregation period must be one of [30, 60].') and the persisted value is unchanged, while 60 succeeds and persists.
Emulator: UpdateMonitor answers error code 'NotImplemented' for both the invalid and the valid value. A scaffold-stub issue for UpdateMonitor (services/service-com-amazonaws-networkmonitor-networkmonitor/src/ops/update_monitor.rs) is being filed alongside this sweep; this cause records the behavioral contract that handler must satisfy — the enumerated-value rejection and the no-mutation guarantee — which the stub issue does not state.
Why: callers get an unmodeled error for a documented validation failure, and cannot tell a rejected update from a transport failure.
Fix: the UpdateMonitor handler and its aggregationPeriod validation (accept only 30/60, leave stored state untouched on rejection).
Traces: networkmonitor.update-monitor-invalid-aggregation-period-rejected (expected ValidationException, got NotImplemented).

## Evidence
Suite: simparity-behavioral-aws-v1, networkmonitor 4/41 passed (37 failed; 12 mismatched, 25 errored).

Cause 1: networkmonitor.create-monitor-initial-state-pending — AssertionError: expected INACTIVE for a zero-probe monitor, got 'PENDING'. networkmonitor.list-monitors-state-filter — Check timed out after 60.0s while polling GetMonitor for a state != PENDING.

Cause 2: networkmonitor.create-monitor-invalid-name-pattern-rejected — AssertionError: expected BadRequestException, got 'ValidationException' (monitorName with spaces and '!'). networkmonitor.monitor-name-length-boundary-rejected — expected BadRequestException, got 'ValidationException' ('a'*201). networkmonitor.monitor-aggregation-period-below-minimum-rejected — expected BadRequestException, got 'ValidationException' (aggregationPeriod=10, parameter_validation disabled so the value reaches the wire).

Cause 3: networkmonitor.get-monitor-not-found — expected message 'No existing monitor found for account 000000000000 and monitorName sim2real-nm-missing-3aba3de49b8d.', got 'monitor sim2real-nm-missing-3aba3de49b8d does not exist'. Same divergence from DeleteMonitor (networkmonitor.delete-monitor-not-found) and from GetMonitor/UpdateMonitor after a delete (networkmonitor.delete-monitor-removes-from-list-monitors). Error code ResourceNotFoundException was correct in all three.

Cause 4: networkmonitor.create-monitor-duplicate-name-conflict-rejected — expected message 'Existing monitor found for account 000000000000 and monitorName sim2real-nwm-mon-252601e601db.', got 'monitor sim2real-nwm-mon-252601e601db already exists (arn:aws:networkmonitor:us-west-2:000000000000:monitor/sim2real-nwm-mon-252601e601db)'.

Cause 5: networkmonitor.probe-invalid-source-arn-rejected — AssertionError: expected CreateProbe to be rejected, but it SUCCEEDED and returned HTTPStatusCode 200 with {'probeId': 'probe-18e0b7d5f4a69c8c00000001', 'sourceArn': 'arn:aws:sns:us-west-2:000000000000:not-a-subnet-topic', 'protocol': 'ICMP', 'packetSize': 56, 'state': 'PENDING'}. Expected ValidationException ('The specified subnet not-a-subnet-topic does not exist or could not be found.') and no probe attached.

Cause 6: networkmonitor.probe-id-pattern-validation-error — AssertionError: expected ResourceNotFoundException, got 'NotImplemented' for GetProbe(monitorName=<existing>, probeId='probe-' + 'a'*25).

Cause 7: networkmonitor.update-monitor-invalid-aggregation-period-rejected — AssertionError: expected ValidationException, got NotImplemented for UpdateMonitor(aggregationPeriod=45).

## Steps to Reproduce
Boot the emulator and point boto3's networkmonitor client at it (region us-west-2).

Cause 1: create_monitor(monitorName=N, aggregationPeriod=60) -> observe response['state'] == 'PENDING' (expected 'INACTIVE'); then poll get_monitor(monitorName=N)['state'] — it never leaves PENDING.
Cause 2: with Config(parameter_validation=False), create_monitor(monitorName='bad name!', aggregationPeriod=60); create_monitor(monitorName='a'*201, aggregationPeriod=30); create_monitor(monitorName=N, aggregationPeriod=10) -> each returns error code ValidationException, expected BadRequestException.
Cause 3: get_monitor(monitorName='never-created') and delete_monitor(monitorName='never-created') -> ResourceNotFoundException with message 'monitor <name> does not exist' instead of 'No existing monitor found for account <acct> and monitorName <name>.'
Cause 4: create_monitor(monitorName=N, aggregationPeriod=60) then create_monitor(monitorName=N, aggregationPeriod=30) -> ConflictException whose message is 'monitor <N> already exists (<arn>)' instead of 'Existing monitor found for account <acct> and monitorName <N>.'
Cause 5: create_monitor(monitorName=M, aggregationPeriod=30) then create_probe(monitorName=M, probe={'sourceArn': 'arn:aws:sns:<region>:<acct>:not-a-subnet-topic', 'destination': '203.0.113.10', 'protocol': 'ICMP', 'packetSize': 56}) -> 200 and a created probe; expected ValidationException and get_monitor(M)['probes'] == [].
Cause 6: get_probe(monitorName=M, probeId='probe-' + 'a'*25) -> NotImplemented; expected ResourceNotFoundException. get_probe(..., probeId='not-a-valid-probe-id') -> expected ValidationException.
Cause 7: create_monitor(monitorName=N, aggregationPeriod=30) then update_monitor(monitorName=N, aggregationPeriod=45) -> NotImplemented; expected ValidationException and get_monitor(N)['aggregationPeriod'] still 30, with update_monitor(..., aggregationPeriod=60) succeeding.
