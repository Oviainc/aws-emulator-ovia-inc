Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmonitor.delete-monitor-with-active-probe-rejected networkmonitor.delete-probe-while-pending-rejected networkmonitor.probes-per-subnet-quota-exceeded-rejected networkmonitor.tag-operations-nonexistent-arn-not-found networkmonitor.delete-probe-becomes-unreachable networkmonitor.update-probe-invalid-state-value-rejected networkmonitor.update-probe-tcp-missing-port-rejected networkmonitor.missing-iam-permission-create-monitor-rejected
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor.delete-monitor-with-active-probe-rejected|_error#8|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: networkmonitor.delete-probe-while-pending-rejected|_error#7|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: networkmonitor.probes-per-subnet-quota-exceeded-rejected|_expect_rejected#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {resp!r}")
Issue-Key: networkmonitor.tag-operations-nonexistent-arn-not-found|run#12|AssertionError|assert err_list["Code"] == "NotFoundException", f"expected NotFoundException, got {err_list['Code']!r}"
Issue-Key: networkmonitor.delete-probe-becomes-unreachable|run#17|AssertionError|assert mutate["Code"] == "ResourceNotFoundException", (
Issue-Key: networkmonitor.update-probe-invalid-state-value-rejected|run#10|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException, got {err['Code']!r}"
Issue-Key: networkmonitor.update-probe-tcp-missing-port-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: networkmonitor.missing-iam-permission-create-monitor-rejected|_poll_create_until#25|Timeout|time.sleep(interval_s)
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# networkmonitor accepts calls AWS refuses (monitor/probe delete guards, per-subnet probe quota, IAM deny) and misclassifies not-found and UpdateProbe refusals (6 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMonitor

## Description
Six distinct root causes in the NetworkMonitor emulator, all grounded in failing behavioral checks. Five are over-permissive paths (the emulator accepts what AWS refuses); one is a wrong error code. Six causes must be closed before this issue is submitted.

=== Cause 1 of 6: DeleteMonitor deletes a monitor that still has an attached probe ===
AWS: DeleteMonitor refuses to delete a monitor while any probe attached to it is not inactive. Recorded behavior only (the Smithy model documents no delete guard: DeleteMonitor's doc is just "Deletes a specified monitor. This action requires the monitorName parameter.", and its per-op error list is only ResourceNotFoundException). The recording shows the refusal as HTTP 400 with error code ValidationException (Smithy: com.amazonaws.networkmonitor#ValidationException, errorKind client, httpError 400, a service-common error applying to every operation). Message is an example only: "All probes on monitor <monitorName> must be inactive before deletion." After the refusal both the monitor and its probe are still readable via GetMonitor/GetProbe; once the probe is deleted the same DeleteMonitor succeeds and GetMonitor then 404s ResourceNotFoundException.
Emulator: DeleteMonitor returns HTTP 200 with an empty JSON body and deletes the monitor even though a probe created seconds earlier is still attached (and still PENDING).
Why: a client that relies on AWS's guard loses the ordered teardown contract. Code written to catch ValidationException and delete probes first sees a success instead, and a monitor is destroyed with live probes still referencing it, so probe records are orphaned/silently dropped rather than refused. Tests that assert "delete is refused while probes exist" pass against AWS and fail here.
Fix: DeleteMonitor's handler must apply a pre-delete state guard over the monitor's attached probes before mutating state, and emit ValidationException (400) when any attached probe is not inactive.
Traces: networkmonitor.delete-monitor-with-active-probe-rejected.

=== Cause 2 of 6: DeleteProbe deletes a probe that is still in the transient PENDING state ===
AWS: DeleteProbe refuses a probe whose state is still PENDING; the probe must leave PENDING (reach ACTIVE) first. Recorded behavior only (the model declares no state precondition; DeleteProbe's per-op errors are ResourceNotFoundException and ServiceQuotaExceededException, with ValidationException service-common). The recording shows HTTP 400, error code ValidationException; message example: "Probe <probeId> must not be in pending state before deletion." The probe survives the refused delete (GetProbe still reports a state that is neither DELETING nor DELETED). Once the probe converges to ACTIVE, DeleteProbe is accepted and GetProbe eventually 404s ResourceNotFoundException.
Emulator: DeleteProbe on a freshly created probe (whose CreateProbe response correctly reported state PENDING) returns HTTP 200 with an empty body and removes it.
Why: the PENDING guard is the only signal clients have that a probe is not yet settled. Without it, teardown helpers that poll for "not PENDING" before deleting are never exercised, and callers cannot distinguish "delete accepted" from "delete refused because the resource is still being provisioned" — an emulator-only success here hides a real-AWS failure path in client code.
Fix: DeleteProbe's handler must gate on the probe's current state and refuse with ValidationException (400) while the probe is PENDING, leaving the probe untouched.
Traces: networkmonitor.delete-probe-while-pending-rejected.

=== Cause 3 of 6: the per-subnet probes-per-monitor quota is not enforced by CreateProbe ===
AWS: the default service quota "Number of probes per subnet for each monitor" is 4. Creating a 5th probe whose probe.sourceArn is the same subnet within the same monitor is refused. Recorded behavior only for the classification: despite being a quota, AWS does NOT return ServiceQuotaExceededException here (Smithy does list ServiceQuotaExceededException, httpError 402, on CreateProbe) — the recording shows HTTP 400 with error code ValidationException; message example: "Maximum probes per subnet limit exceeded." The 5th probe is not created (GetMonitor still lists exactly 4 probes), and a probe sourced from a different subnet in the same monitor is still accepted.
Emulator: the 5th CreateProbe on the same sourceArn returns HTTP 200 and a fully populated probe (probeId/probeArn allocated, state PENDING), i.e. no per-subnet counting at all.
Why: clients and capacity-planning code that handle the quota refusal never see it; an emulator-backed test suite silently allows configurations real AWS rejects, and the per-subnet dimension of the limit (the fact that a different subnet is unaffected) is untested. Note the classification detail: a client catching ServiceQuotaExceededException would also be wrong — the refusal must be ValidationException/400.
Fix: CreateProbe's validation/state must count existing probes grouped by (monitorName, probe.sourceArn) and refuse the 5th with ValidationException (400) without allocating a probe id.
Traces: networkmonitor.probes-per-subnet-quota-exceeded-rejected.

=== Cause 4 of 6: tagging operations report the wrong error code for an unknown resourceArn ===
AWS: ListTagsForResource, TagResource and UntagResource given a well-formed networkmonitor ARN in the caller's own account that corresponds to no existing monitor or probe are refused with error code NotFoundException (recorded behavior; message example "Resource was not found"). The Smithy model declares ResourceNotFoundException (httpError 404) for all three operations, but the service's tagging facade returns the generic NotFoundException on the wire, which is what the check pins.
Emulator: returns error code ResourceNotFoundException for ListTagsForResource with an unknown ARN (the check fails at the first of the three assertions, so TagResource/UntagResource are presumed to share the mapping and must be fixed together).
Why: the error code is part of the contract — botocore/SDK error handling, retry classifiers and `except ClientError` branches switch on Error.Code. A client catching NotFoundException around tag calls (as it must against real AWS) sees an unhandled ResourceNotFoundException here, so an emulator-passing client breaks on AWS and vice versa.
Why-note: positive control passes — tagging a real monitor ARN and reading the tags back round-trips correctly, so only the not-found classification is wrong.
Fix: the tagging operations' error mapping (ListTagsForResource / TagResource / UntagResource handlers) must emit NotFoundException for an unresolvable resourceArn rather than the modeled ResourceNotFoundException.
Traces: networkmonitor.tag-operations-nonexistent-arn-not-found.

=== Cause 5 of 6: UpdateProbe has no refusal semantics — it answers NotImplemented for not-found and for invalid input ===
AWS: UpdateProbe (PATCH /monitors/{monitorName}/probes/{probeId}) refuses in three distinct ways that the emulator currently cannot express:
  (a) a probeId that no longer resolves (the probe was deleted) -> ResourceNotFoundException (Smithy: declared on UpdateProbe, errorKind client, httpError 404); the same code and message GetProbe returns for that id, i.e. a dead probe id is unusable for mutation, not revivable.
  (b) state=ERROR -> ValidationException (400). Smithy: UpdateProbeInput.state targets ProbeState whose enum values are PENDING/ACTIVE/INACTIVE/ERROR/DELETING/DELETED, so ERROR passes enum validation, but only ACTIVE and INACTIVE are settable target states; message example "ERROR is not a valid probe status." The probe's state must be unchanged after the refusal, and state=INACTIVE must still be accepted.
  (c) protocol=TCP supplied without destinationPort -> ValidationException (400). Smithy documents the cross-member rule on UpdateProbeInput.protocol: "This can be either TCP or ICMP. If the protocol is TCP, then port is also required", and on destinationPort: "required only if the protocol is TCP". The probe's existing ICMP configuration must be untouched after the refusal, and protocol=TCP plus destinationPort=443 together must be accepted and reflected by a subsequent GetProbe.
Emulator: every UpdateProbe call answers error code NotImplemented ("operation UpdateProbe is scaffolded but not implemented in this emulator"), so all three refusals, and the accompanying no-mutation and positive-control guarantees, are absent.
Why: callers cannot distinguish a dead probe id from a bad parameter from a working update; a not-found that arrives as NotImplemented is unretryable and unhandled by SDK error branches, and the "rejected update leaves the probe unchanged" invariant is unverifiable.
Fix: implement UpdateProbe's handler together with its validation (settable-state whitelist, TCP-requires-destinationPort cross-member rule) and its state lookup (not-found for an unresolvable probeId), so the refusals land as ResourceNotFoundException/ValidationException. Related open issue: the separately filed "Implement networkmonitor UpdateProbe: unimplemented scaffold stub" work item covers only standing the operation up; it does not name these three refusal requirements or the no-mutation invariant, which is why they are recorded here.
Traces: networkmonitor.delete-probe-becomes-unreachable (case a; the preceding DeleteProbe/GetProbe 404 path passes, only the mutation-after-delete assertion diverges), networkmonitor.update-probe-invalid-state-value-rejected (case b), networkmonitor.update-probe-tcp-missing-port-rejected (case c).

=== Cause 6 of 6: CreateMonitor is not authorized against the caller's identity-based policy (explicit Deny ignored) ===
AWS: CreateMonitor is authorized against the calling identity's IAM policies. With an inline Allow for networkmonitor:CreateMonitor the assumed-role caller succeeds; once an explicit Deny for the same action is attached on top, the identical call is refused with AccessDeniedException (Smithy: com.amazonaws.networkmonitor#AccessDeniedException, errorKind client, httpError 403, a service-common error) and creates no monitor — a follow-up GetMonitor by the admin identity returns ResourceNotFoundException (404). Removing the Deny restores success for the same identity. The AccessDenied message is built from the request and the principal ("User: <assumed-role arn>/<session> is not authorized to perform: networkmonitor:CreateMonitor on resource: <monitor arn> with an explicit deny in an identity-based policy"), so that construction — principal, action and resource — is the requirement, not any fixed text.
Emulator: the check never converges and is killed by the harness after 60s: it polls CreateMonitor under the probed role waiting for the authorization verdict to change and the verdict never changes. Hypothesis: networkmonitor does not consult the caller's identity-based policies at all, so the explicit Deny is ignored and CreateMonitor keeps succeeding (the symmetric failure — an allowed caller being refused — would produce the same non-convergence and must be ruled out while fixing).
Why: an emulator that answers every caller as authorized cannot be used to test least-privilege policies or deny-based guardrails; code paths that handle AccessDeniedException (403) for networkmonitor are never exercised, and a policy change that would break a client in production looks harmless here.
Fix: CreateMonitor's request authorization must evaluate the signing identity's identity-based policies (explicit Deny winning over Allow) and refuse with AccessDeniedException (403) before any monitor state is created.
Traces: networkmonitor.missing-iam-permission-create-monitor-rejected.

## Evidence
Suite: networkmonitor 26/41 passed (63%), 15 failed. Per-cause expected vs actual:

1) delete-monitor-with-active-probe-rejected — expected: ValidationException (400), message e.g. "All probes on monitor <name> must be inactive before deletion."; actual: "expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}}}" (DeleteMonitor returned 200 / empty body with a probe attached).

2) delete-probe-while-pending-rejected — expected: ValidationException (400), message e.g. "Probe <probeId> must not be in pending state before deletion."; actual: "expected this call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}}" (DeleteProbe accepted on a probe the emulator itself had just reported as PENDING).

3) probes-per-subnet-quota-exceeded-rejected — expected: ValidationException (400), message e.g. "Maximum probes per subnet limit exceeded." for the 5th probe on one subnet; actual: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'probeId': 'probe-...', 'probeArn': 'arn:aws:networkmonitor:...:probe/probe-...', 'sourceArn': 'arn:aws:ec2:...:subnet/subnet-...', 'destination': '203.0.113.99', 'protocol': 'ICMP', 'packetSize': 56, 'addressFamily': 'IPV4', 'state': 'PENDING', ...}" (ids/timestamps are environment-specific; the divergence is the 200).

4) tag-operations-nonexistent-arn-not-found — expected Error.Code "NotFoundException"; actual "AssertionError: expected NotFoundException, got 'ResourceNotFoundException'" on ListTagsForResource with a non-existent monitor ARN in the caller's own account. Positive control (tag/list/untag on a real monitor ARN) passed.

5a) delete-probe-becomes-unreachable — DeleteProbe and the GetProbe 404 (ResourceNotFoundException + message) passed; then "AssertionError: mutating a probe id that no longer resolves must fail not-found, got 'NotImplemented'" for UpdateProbe(state=INACTIVE) on the deleted probeId.
5b) update-probe-invalid-state-value-rejected — "AssertionError: expected ValidationException, got 'NotImplemented'" for UpdateProbe(state="ERROR") on an ACTIVE probe.
5c) update-probe-tcp-missing-port-rejected — "AssertionError: expected ValidationException, got NotImplemented" for UpdateProbe(protocol="TCP") with no destinationPort.
Underlying emulator response for all three: "An error occurred (NotImplemented) when calling the UpdateProbe operation: operation UpdateProbe is scaffolded but not implemented in this emulator".

6) missing-iam-permission-create-monitor-rejected — expected AccessDeniedException (403) on CreateMonitor once an explicit Deny for networkmonitor:CreateMonitor is attached to the assumed role, then ResourceNotFoundException from GetMonitor showing no monitor was created, then success restored when the Deny is removed; actual: "Check timed out after 60.0s" while polling for the authorization verdict to change — the verdict never changed.

## Steps to Reproduce
All steps use the networkmonitor client against the emulator (us-west-2, account 000000000000 in the replay environment).

1) DeleteMonitor guard: CreateMonitor(monitorName=M, aggregationPeriod=60); CreateProbe(monitorName=M, probe={sourceArn=<subnet arn>, destination=192.168.64.64, protocol=ICMP, packetSize=56}); call DeleteMonitor(monitorName=M). Expect ValidationException/400; observed HTTP 200 and the monitor gone.

2) DeleteProbe PENDING guard: CreateMonitor(M, 30); CreateProbe(... protocol=ICMP ...) and confirm the response says state=PENDING; immediately call DeleteProbe(monitorName=M, probeId=P). Expect ValidationException/400 and the probe still present; observed HTTP 200.

3) Per-subnet quota: CreateMonitor(M, 30); create 4 probes all with the SAME probe.sourceArn (one subnet) in M; create a 5th with that same sourceArn. Expect ValidationException/400 "Maximum probes per subnet limit exceeded." and GetMonitor(M) still listing 4 probes; observed HTTP 200 and a 5th probe allocated. (Control: a probe on a different subnet must still be accepted.)

4) Tagging not-found: ListTagsForResource(resourceArn="arn:aws:networkmonitor:<region>:<own account>:monitor/<name that does not exist>"). Expect Error.Code NotFoundException; observed ResourceNotFoundException. Repeat with TagResource(resourceArn=<same>, tags={k:v}) and UntagResource(resourceArn=<same>, tagKeys=[k]).

5) UpdateProbe refusals: CreateMonitor(M, 30); CreateProbe(... protocol=ICMP, packetSize=56 ...); poll GetProbe until state != PENDING.
   5a) DeleteProbe(M, P), poll GetProbe until it 404s, then UpdateProbe(monitorName=M, probeId=P, state="INACTIVE") — expect ResourceNotFoundException/404; observed NotImplemented.
   5b) On a live ACTIVE probe: UpdateProbe(M, P, state="ERROR") — expect ValidationException/400 and GetProbe state still ACTIVE; observed NotImplemented. Then UpdateProbe(M, P, state="INACTIVE") must be accepted.
   5c) On a live ICMP probe: UpdateProbe(M, P, protocol="TCP") with no destinationPort — expect ValidationException/400 and GetProbe protocol still ICMP; observed NotImplemented. Then UpdateProbe(M, P, protocol="TCP", destinationPort=443) must be accepted and reflected by GetProbe.

6) IAM deny: iam.CreateRole with a trust policy allowing the account root to sts:AssumeRole; iam.PutRolePolicy granting Allow networkmonitor:CreateMonitor on *; sts.AssumeRole and build a networkmonitor client from the returned credentials; CreateMonitor under that identity succeeds (control). Then iam.PutRolePolicy adding Deny networkmonitor:CreateMonitor on * and retry the identical CreateMonitor: expect AccessDeniedException/403 and GetMonitor (admin identity) to report ResourceNotFoundException for the attempted name. Observed: the call keeps succeeding and the check is killed at the 60s harness timeout. Remove the Deny and the call must succeed again for the same identity.
