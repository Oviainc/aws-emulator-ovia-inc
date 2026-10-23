Issue-Source: trace-sweep-divergence
Issue-Trace: networkmonitor-monitor-lifecycle-update-tags
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkmonitor-monitor-lifecycle-update-tags networkmonitor-error-paths-probe-v2
Issue-Shape-Id: com.amazonaws.networkmonitor#NetworkMonitor
Issue-Key: networkmonitor|TagResource|2xx->2xx|status
Issue-Key: networkmonitor|UntagResource|2xx->2xx|status
Issue-Key: networkmonitor|GetProbe|4xx->4xx|$.message
Issue-Commit: a91eb31cf888327ed8ad916c03309e3893f84f70

# networkmonitor response-code and validation-message fidelity: tagging ops answer 200 not 204, anchored pattern echoed

> _Note: this issue was identified against the code base at commit `a91eb31cf888327ed8ad916c03309e3893f84f70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three distinct root causes in NetworkMonitor's REST response construction, found by replaying the wire corpus (3 of 23 steps mismatched; no step blocked or errored). Causes 1 and 2 are the HTTP success status of the two tagging operations; cause 3 is the rendering of a modeled @pattern inside the standard validation-error message. THREE causes must be closed before this issue is submitted.

=== Cause 1 of 3: TagResource answers 200 instead of 204 ===
AWS: TagResource returns an empty body and HTTP 204 No Content. Recorded behavior only for the status: Smithy (com.amazonaws.networkmonitor#TagResource) binds POST /tags/{resourceArn} with an EMPTY output shape (TagResourceOutput has no members) and NO explicit @http code — the 200 reported by the model is the protocol default (codeDefaulted: true), so the model does not state the success status and the recording is the authority. The recording shows AWS answering 204 with no body for a successful tagging of a monitor ARN.
Emulator: answers HTTP 200 (empty body) for the same successful call. Status is the only divergence; no body keys differ.
Why: 204 vs 200 is part of the wire contract. A client that branches on the response status (or a proxy/cache layer that treats 200 as carrying an entity while 204 is defined to carry none) sees a response AWS never emits; conformance harnesses and SDK/HTTP-level assertions on the no-content status fail against the emulator even though the operation's effect is correct.
Fix: the TagResource handler's response serializer for the success path must emit 204 with no content, instead of defaulting to 200 because the output shape is empty.
Traces: networkmonitor-monitor-lifecycle-update-tags#task#6.

=== Cause 2 of 3: UntagResource answers 200 instead of 204 ===
AWS: UntagResource likewise returns an empty body and HTTP 204 No Content. Recorded behavior only for the status: Smithy binds DELETE /tags/{resourceArn} (tagKeys via @httpQuery) with an EMPTY output shape (UntagResourceOutput, no members) and no explicit @http code (codeDefaulted: true). The recording shows 204 with no body on a successful untag.
Emulator: answers HTTP 200 (empty body). Status is the only divergence.
Why: same as cause 1 — the documented/recorded no-content status is what callers and intermediaries key on, and the emulator never produces it for either tagging operation. The two causes are the same defect shape but live on two distinct handlers/routes (POST vs DELETE on /tags/{resourceArn}), so fixing one does not fix the other unless the empty-output success-code rule is made shared.
Fix: the UntagResource handler's success-path serializer must emit 204 with no content.
Traces: networkmonitor-monitor-lifecycle-update-tags#task#8.

=== Cause 3 of 3: the ValidationException constraint message echoes the probeId @pattern WITH its regex anchors ===
AWS: a probeId that violates its pattern is refused with HTTP 400 / ValidationException, using AWS's standard constraint template built from the request and the modeled constraint: "1 validation error detected: Value '<value>' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>". Smithy: GetProbeInput.probeId targets com.amazonaws.networkmonitor#ProbeId whose @pattern is "^probe-[a-z0-9A-Z-]{21,64}$"; GetProbe models ValidationException with httpError 400. The recording shows AWS interpolating that pattern ANCHOR-STRIPPED — "probe-[a-z0-9A-Z-]{21,64}" — so the rule producing the message is known (request-derived template plus the modeled constraint with the anchoring ^ and $ removed) and the rendering is therefore a requirement, not an incidental recorded string.
Emulator: status 400 and error code ValidationException already match; it interpolates the model's pattern verbatim, including the anchors: "... Member must satisfy regular expression pattern: ^probe-[a-z0-9A-Z-]{21,64}$". The offending value, the count and the member path 'probeId' all match.
Why: the echoed pattern is the one machine-readable description of the constraint the caller receives. Tooling that surfaces, matches on, or copies the pattern into a validator gets a regex AWS never emits, and the anchors leak a model detail AWS strips. Because the template is shared, the defect applies to every member refused for a @pattern violation, not just probeId.
Fix: the shared constraint-violation message formatter in input validation must strip the anchoring ^ and $ from a @pattern before interpolating it into the "Member must satisfy regular expression pattern:" clause.
Traces: networkmonitor-error-paths-probe-v2#task#4.

Related open issue, which does NOT cover any of the above: "networkmonitor validation-error messages diverge from AWS: anchored regex echoed in the constraint message..." (20261023T004257Z-networkmonitor-validation-error-messages-diverge-from-aws-anchored.md) describes the same root cause as cause 3, but it is a BEHAVIORAL issue keyed on check ids (networkmonitor.probe-id-pattern-validation-error) and carries none of this corpus's replay keys — the replay reported no owner for this step. Cause 3 is recorded here so the wire fixture above is actually re-replayed; it may be closed together with that issue only if this fixture step then passes. The tagging status causes (1 and 2) appear in no open issue.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 23 steps, 20 passed, 3 mismatched, 0 errored, 0 blocked, score 87.0%; emulator_up=true, bootstrap_signal=false, state_reset_failures=0 (so none of these ran against stale state).

1) Cause 1 — fixture networkmonitor-monitor-lifecycle-update-tags, step task#6, operation TagResource, verdict mismatch. divergence_key service=networkmonitor|TagResource|status=204->200|keys=status. Detail: "status: expected 204, got 200". Expected status 204, actual 200.

2) Cause 2 — fixture networkmonitor-monitor-lifecycle-update-tags, step task#8, operation UntagResource, verdict mismatch. divergence_key service=networkmonitor|UntagResource|status=204->200|keys=status. Detail: "status: expected 204, got 200". Expected status 204, actual 200.

3) Cause 3 — fixture networkmonitor-error-paths-probe-v2, step task#4, operation GetProbe, verdict mismatch. divergence_key service=networkmonitor|GetProbe|status=400->400|keys=$.message. Status matched (400 expected, 400 actual); only $.message diverged: expected "1 validation error detected: Value 'probe-short' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: probe-[a-z0-9A-Z-]{21,64}", got "1 validation error detected: Value 'probe-short' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^probe-[a-z0-9A-Z-]{21,64}$". ('probe-short' is the recording's example value; the requirement is the rendering rule, not that value.) Model cross-check: ProbeId @pattern = "^probe-[a-z0-9A-Z-]{21,64}$".

Model cross-check for causes 1 and 2: TagResource -> POST /tags/{resourceArn}, output shape TagResourceOutput with no members, http code 200 flagged codeDefaulted; UntagResource -> DELETE /tags/{resourceArn}, output shape UntagResourceOutput with no members, http code 200 flagged codeDefaulted. Neither declares an explicit success code, so the recorded 204 is the contract.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus) against the emulator built from this checkout; the three steps below are the mismatches.

1) Cause 1 (TagResource -> 204): replay fixture networkmonitor-monitor-lifecycle-update-tags up to step task#6 — create a monitor, then POST /tags/{monitorArn} with a tag map. Expect HTTP 204 with an empty body; observed HTTP 200 with an empty body.

2) Cause 2 (UntagResource -> 204): continue the same fixture to step task#8 — DELETE /tags/{monitorArn}?tagKeys=<key>. Expect HTTP 204 with an empty body; observed HTTP 200 with an empty body.

3) Cause 3 (anchor-stripped pattern in the constraint message): replay fixture networkmonitor-error-paths-probe-v2 to step task#4 — GET /monitors/{monitorName}/probes/{probeId} with a probeId that violates the ProbeId pattern (the recording uses 'probe-short'). Expect HTTP 400, code ValidationException, message "1 validation error detected: Value '<value>' at 'probeId' failed to satisfy constraint: Member must satisfy regular expression pattern: probe-[a-z0-9A-Z-]{21,64}" (pattern rendered without ^ and $); observed the same status and code but the pattern rendered as "^probe-[a-z0-9A-Z-]{21,64}$".

After fixing, re-replay both fixtures and confirm all 23 steps pass.
