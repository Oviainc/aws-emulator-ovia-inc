Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkmonitor.probe-id-pattern-validation-error networkmonitor.create-probe-tcp-missing-port-rejected networkmonitor.update-monitor-invalid-aggregation-period-rejected
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor.probe-id-pattern-validation-error|run#13|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: networkmonitor.create-probe-tcp-missing-port-rejected|run#9|AssertionError|assert err["Message"] == expected, f"expected message {expected!r}, got {err['Message']!r}"
Issue-Key: networkmonitor.update-monitor-invalid-aggregation-period-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# networkmonitor validation-error messages diverge from AWS: anchored regex echoed in the constraint message, and two rule-determined refusal messages reworded (3 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProbe

## Description
Three distinct root causes, all in NetworkMonitor's validation-error message construction. In each case the HTTP status and the error code already match AWS (ValidationException, 400) and only the message text diverges — but in each case the rule that produces the message is known (the standard AWS constraint-violation template built from the request and the modeled constraint, or a fixed rule-determined sentence corroborated by two independent recordings), so the message is part of the contract rather than an incidental recorded value. Three causes must be closed before this issue is submitted.

=== Cause 1 of 3: the constraint-violation message echoes the probeId pattern WITH regex anchors ===
AWS: a probeId that does not match its pattern is refused with ValidationException (400) using AWS's standard constraint template, which is built from the request and the modeled constraint: "1 validation error detected: Value '<value>' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Smithy: GetProbeInput/UpdateProbeInput.probeId targets com.amazonaws.networkmonitor#ProbeId, whose @pattern is "^probe-[a-z0-9A-Z-]{21,64}$". The recording shows AWS rendering that pattern in the message WITHOUT the leading ^ and trailing $: "probe-[a-z0-9A-Z-]{21,64}".
Emulator: renders the pattern verbatim from the model including the anchors: "... Member must satisfy regular expression pattern: ^probe-[a-z0-9A-Z-]{21,64}$". Everything else about the response (status 400, code ValidationException, the count, the offending value, the member path 'probeId') matches.
Why: this message is the one machine-readable description of the constraint AWS gives a caller — tooling and tests that surface or match the echoed pattern (and users who copy it into a validator) get a pattern string that AWS never emits, and the anchors leak a model detail AWS strips. Because the whole template is request-derived, a general fix here corrects every member whose constraint is reported this way, not just probeId.
Fix: the shared constraint-violation message formatter used by input validation must strip the anchoring ^ and $ from a @pattern before interpolating it into the "Member must satisfy regular expression pattern:" clause.
Traces: networkmonitor.probe-id-pattern-validation-error.

=== Cause 2 of 3: CreateProbe's TCP-requires-destinationPort refusal is reworded ===
AWS: CreateProbe with probe.protocol=TCP and no probe.destinationPort is refused with ValidationException (400). Smithy states the rule: ProbeInput.destinationPort is documented "required only if the protocol is TCP", and UpdateProbeInput.protocol as "If the protocol is TCP, then port is also required"; ProbeInput.protocol targets Protocol (enum TCP, ICMP). The message AWS emits for that rule is the fixed sentence "Destination port must be provided for TCP protocol." — rule-determined rather than incidental: the same sentence is recorded for the same rule on the UpdateProbe path.
Emulator: refuses correctly with ValidationException (400) but with its own wording, "destinationPort is required when protocol is TCP".
Why: the status and code are right, so clients branching on Error.Code are unaffected; what breaks is anything surfacing or asserting the service's message — parity tests, golden-file comparisons and user-facing error reporting differ from AWS for a refusal whose text is stable and rule-determined.
Why-note: the rest of the check is blocked behind this assertion, so two further AWS guarantees for this path are unverified and must also hold once the message is fixed: the rejected TCP probe must not be attached to the monitor (GetMonitor reports zero probes), and an ICMP probe (which needs no port) must still succeed on that same monitor.
Fix: CreateProbe's validation for the protocol/destinationPort cross-member rule must emit AWS's wording for that rule.
Traces: networkmonitor.create-probe-tcp-missing-port-rejected. (The same rule on UpdateProbe is recorded by networkmonitor.update-probe-tcp-missing-port-rejected, which currently cannot reach the message because UpdateProbe answers NotImplemented — that is filed separately as the UpdateProbe refusal-semantics cause.)

=== Cause 3 of 3: UpdateMonitor's aggregationPeriod refusal is reworded and leaks the offending value ===
AWS: UpdateMonitor accepts only aggregationPeriod 30 or 60. Smithy states the rule in UpdateMonitorInput.aggregationPeriod's doc — "The aggregation time, in seconds, to change to. This must be either 30 or 60" — while the shape's @range only pins min 30 (AggregationPeriod: long, range min 30), so the two-value allow-list is enforced by the operation, not the range. An out-of-range value such as 45 is refused with ValidationException (400) and the rule-determined sentence "The aggregation period must be one of [30, 60]."
Emulator: refuses correctly with ValidationException (400) but with a reworded message that additionally interpolates the submitted value: "aggregationPeriod must be one of [30, 60], got 45".
Why: as above, the code and status are right and only the text diverges; AWS's sentence does not echo the rejected value, so parity assertions and error-text handling diverge for a stable, rule-determined refusal.
Why-note: the remainder of the check is blocked behind this assertion, so two further guarantees for this path are unverified and must also hold: the rejected UpdateMonitor must not mutate the stored aggregationPeriod (GetMonitor still reports 30), and a valid UpdateMonitor to 60 must be accepted and persist.
Fix: UpdateMonitor's validation for the aggregationPeriod allow-list must emit AWS's wording for that rule.
Traces: networkmonitor.update-monitor-invalid-aggregation-period-rejected.

## Evidence
Suite: networkmonitor 26/41 passed (63%), 15 failed. All three failures are message-text-only; status 400 and code ValidationException already match.

1) probe-id-pattern-validation-error — "AssertionError: expected message \"1 validation error detected: Value 'not-a-valid-probe-id' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: probe-[a-z0-9A-Z-]{21,64}\", got \"1 validation error detected: Value 'not-a-valid-probe-id' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^probe-[a-z0-9A-Z-]{21,64}$\"". Model cross-check: ProbeId @pattern = "^probe-[a-z0-9A-Z-]{21,64}$" — AWS renders it anchor-stripped, the emulator renders it verbatim.

2) create-probe-tcp-missing-port-rejected — code assertion passed (ValidationException); "AssertionError: expected message 'Destination port must be provided for TCP protocol.', got 'destinationPort is required when protocol is TCP'" for CreateProbe(probe={sourceArn, destination, protocol: TCP, packetSize: 56}) with no destinationPort.

3) update-monitor-invalid-aggregation-period-rejected — code assertion passed (ValidationException); "AssertionError: expected message 'The aggregation period must be one of [30, 60].', got 'aggregationPeriod must be one of [30, 60], got 45'" for UpdateMonitor(monitorName=M, aggregationPeriod=45).

Corroboration that cause 2's sentence is rule-determined and not an incidental recording: a second, independent check (update-probe-tcp-missing-port-rejected) records the identical expected sentence "Destination port must be provided for TCP protocol." for the same cross-member rule on the UpdateProbe path.

## Steps to Reproduce
1) Anchored pattern in the constraint message: call GetProbe(monitorName=<any valid name>, probeId="not-a-valid-probe-id"). Expect HTTP 400 ValidationException with message "1 validation error detected: Value 'not-a-valid-probe-id' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: probe-[a-z0-9A-Z-]{21,64}"; observed the same message but with the pattern rendered as "^probe-[a-z0-9A-Z-]{21,64}$".

2) CreateProbe TCP without port: CreateMonitor(monitorName=M, aggregationPeriod=60); CreateProbe(monitorName=M, probe={sourceArn=<subnet arn>, destination="192.168.63.63", protocol="TCP", packetSize=56}) with destinationPort omitted. Expect HTTP 400 ValidationException, message "Destination port must be provided for TCP protocol."; observed message "destinationPort is required when protocol is TCP". Then confirm GetMonitor(M) lists zero probes and that the same CreateProbe with protocol="ICMP" succeeds.

3) UpdateMonitor invalid aggregationPeriod: CreateMonitor(monitorName=M, aggregationPeriod=30); UpdateMonitor(monitorName=M, aggregationPeriod=45). Expect HTTP 400 ValidationException, message "The aggregation period must be one of [30, 60]."; observed "aggregationPeriod must be one of [30, 60], got 45". Then confirm GetMonitor(M).aggregationPeriod is still 30 and that UpdateMonitor(M, 60) is accepted and persists.
