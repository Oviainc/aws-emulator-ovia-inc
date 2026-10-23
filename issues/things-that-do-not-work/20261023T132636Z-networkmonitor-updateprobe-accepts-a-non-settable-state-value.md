Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmonitor.update-probe-invalid-state-value-rejected networkmonitor.update-probe-tcp-missing-port-rejected
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor.update-probe-invalid-state-value-rejected|_error#9|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: networkmonitor.update-probe-tcp-missing-port-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# networkmonitor UpdateProbe accepts a non-settable state value (state=ERROR) and persists it, and its TCP-requires-destinationPort refusal message diverges from AWS (2 causes)

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProbe

## Description
Two distinct root causes on the NetworkMonitor UpdateProbe path. Both must be closed before this issue is submitted.

=== Cause 1 of 2: UpdateProbe accepts state=ERROR, returns 200 and writes the probe into that state ===
AWS: UpdateProbe's state member is the documented activate/deactivate toggle — only ACTIVE and INACTIVE are settable target states. Smithy (com-amazonaws-networkmonitor-networkmonitor.json): UpdateProbeInput.state targets com.amazonaws.networkmonitor#ProbeState, an enum whose values are PENDING, ACTIVE, INACTIVE, ERROR, DELETING, DELETED — i.e. the enum also carries lifecycle-only values the service itself assigns (PENDING, ERROR, DELETING, DELETED); UpdateProbeInput.state is documented only as "The state of the probe update", so the allow-list of settable values is enforced by the operation and not by the enum. UpdateProbe models ValidationException (httpError 400). Recorded behavior: sending state="ERROR" for an ACTIVE probe is refused with HTTP 400 ValidationException, the probe's stored state is unchanged (GetProbe still reports ACTIVE), and a genuinely settable value (state=INACTIVE) is still accepted. The recorded message is "ERROR is not a valid probe status." (example of the rule "the submitted state is not a settable probe status").
Emulator: accepts the call. UpdateProbe returns HTTP 200 with a full probe body in which state has been set to the service-assigned lifecycle value: ... 'protocol': 'ICMP', 'state': 'ERROR', 'modifiedAt': <bumped> ... The probe is left persisted in ERROR, so the rejection never happens and the state write is not guarded at all.
Why: a client that relies on AWS refusing non-settable states gets no error at all and, worse, silently corrupts the probe's lifecycle state — a probe parked in ERROR by a caller is indistinguishable from one the service itself failed, so health/alarm logic reading state is wrong, and code paths that handle ValidationException (retry with a corrected value, surface the bad input) are never exercised. Because the write is applied, the divergence is not just a missing error but durable bad state.
Fix: UpdateProbe's input validation must restrict the state member to the settable target states (ACTIVE, INACTIVE) and reject anything else with ValidationException (400) before any mutation is applied; the handler's state transition must not write a service-assigned lifecycle value supplied by the caller.
Traces: networkmonitor.update-probe-invalid-state-value-rejected.
Why-note: the check is blocked at this assertion, so two further AWS guarantees on this path are unverified and must also hold once the rejection exists: the rejected update must leave GetProbe().state == ACTIVE, and the subsequent UpdateProbe(state=INACTIVE) must be accepted and converge to INACTIVE.

=== Cause 2 of 2: UpdateProbe's TCP-requires-destinationPort refusal is reworded ===
AWS: switching a probe's protocol to TCP without also supplying destinationPort is refused with ValidationException (400). Smithy states the rule: UpdateProbeInput.protocol is documented "If the protocol is TCP, then port is also required", ProbeInput.destinationPort is documented "required only if the protocol is TCP", and protocol targets Protocol (enum TCP, ICMP). The message AWS emits for that rule is the fixed sentence "Destination port must be provided for TCP protocol." — rule-determined rather than incidental, since the identical sentence is recorded for the identical cross-member rule on the CreateProbe path.
Emulator: refuses with the right status and code (ValidationException, 400) but with its own wording: "destinationPort is required when protocol is TCP".
Why: clients branching on Error.Code are unaffected; what breaks is anything that surfaces or asserts the service's message for this refusal — parity tests, golden-file comparisons and user-facing error reporting diverge from AWS for a refusal whose text is stable and determined by a documented rule.
Fix: the cross-member protocol/destinationPort validation on the UpdateProbe path must emit AWS's wording for that rule; it should share one message with the CreateProbe path so both emit the same sentence.
Related already-filed issue: 20261023T004257Z-networkmonitor-validation-error-messages-diverge-from-aws-anchored.md covers the SAME wording defect on CreateProbe (its cause 2) and notes that this UpdateProbe check could not then reach the message because UpdateProbe answered NotImplemented. UpdateProbe now answers, the divergence is observable on the UpdateProbe path, and that issue's trace ids do not include this check, so it is filed here; a fixer should fix both paths from one shared message.
Traces: networkmonitor.update-probe-tcp-missing-port-rejected.

## Evidence
Suite: networkmonitor 30/41 passed (73%), 11 failed. Both failures below are the net-new, unowned ones.

1) [mismatch] networkmonitor.update-probe-invalid-state-value-rejected — CreateMonitor + CreateProbe(protocol=ICMP, packetSize=56), wait until state==ACTIVE, then UpdateProbe(monitorName=M, probeId=P, state="ERROR"). Expected: a rejection (HTTP 400, Error.Code ValidationException, message e.g. "ERROR is not a valid probe status."). Observed: "AssertionError: expected a rejection, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'probeId': 'probe-...', 'probeArn': 'arn:aws:networkmonitor:...:probe/probe-...', 'sourceArn': 'arn:aws:ec2:...:subnet/subnet-...', 'destination': '203.0.113.70', 'protocol': 'ICMP', 'packetSize': 56, 'addressFamily': 'IPV4', 'state': 'ERROR', 'createdAt': ..., 'modifiedAt': ..., 'tags': {}}" — i.e. 200 OK and the caller-supplied lifecycle state ERROR persisted onto the probe.
Model cross-check: ProbeState = [PENDING, ACTIVE, INACTIVE, ERROR, DELETING, DELETED]; UpdateProbe errors include ValidationException (400).

2) [mismatch] networkmonitor.update-probe-tcp-missing-port-rejected — UpdateProbe switching an ICMP probe to protocol=TCP with destinationPort omitted. The code/status assertions passed (ValidationException, 400); only the text diverged: "AssertionError: expected message 'Destination port must be provided for TCP protocol.', got 'destinationPort is required when protocol is TCP'". Corroboration that the sentence is rule-determined: networkmonitor.create-probe-tcp-missing-port-rejected records the identical expected sentence for the same rule on CreateProbe.

## Steps to Reproduce
Cause 1 — UpdateProbe accepts a non-settable state:
1. CreateMonitor(monitorName=M, aggregationPeriod=30).
2. CreateProbe(monitorName=M, probe={sourceArn=<subnet arn>, destination="203.0.113.70", protocol="ICMP", packetSize=56}) -> probeId P; poll GetProbe until state leaves PENDING (AWS reaches ACTIVE).
3. UpdateProbe(monitorName=M, probeId=P, state="ERROR"). Expect HTTP 400 with Error.Code ValidationException (AWS message example: "ERROR is not a valid probe status."). Observed HTTP 200 with the probe body and state='ERROR'.
4. GetProbe(monitorName=M, probeId=P) must still report state ACTIVE; observed ERROR.
5. UpdateProbe(monitorName=M, probeId=P, state="INACTIVE") must be accepted and GetProbe must converge to INACTIVE (positive control).

Cause 2 — UpdateProbe TCP without destinationPort:
1. CreateMonitor(monitorName=M, aggregationPeriod=30); CreateProbe(... protocol="ICMP", packetSize=56) -> probeId P; wait until it leaves PENDING.
2. UpdateProbe(monitorName=M, probeId=P, protocol="TCP") with destinationPort omitted. Expect HTTP 400 ValidationException with message "Destination port must be provided for TCP protocol."; observed the same status/code with message "destinationPort is required when protocol is TCP".
3. Confirm the probe's original ICMP configuration is untouched, and that UpdateProbe with protocol="TCP" plus destinationPort together succeeds (positive control).
