Issue-Source: trace-sweep-divergence
Issue-Trace: sagemakerjobruntime-validation-errors
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemakerjobruntime-validation-errors
Issue-Shape-Id: com.amazonaws.sagemakerjobruntime#AgenticRFTRuntimeService
Issue-Key: sagemakerjobruntime|SampleWithResponseStream|4xx->4xx|$.Message
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# ValidationException message truncates the rejected TrajectoryId value instead of echoing it in full

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SampleWithResponseStream

## Description
One cause.

Cause 1 of 1: the @length constraint violation message for the header-bound TrajectoryId member is emitted with the offending value elided, not echoed verbatim (fixture sagemakerjobruntime-validation-errors, step task#2).

AWS: Smithy (services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/smithy-model/com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice.json) models SampleWithResponseStream input member TrajectoryId as required, bound to header X-Amzn-SageMaker-Trajectory-Id, target TrajectoryId with @length {min:1, max:256}; the operation declares ValidationException with httpError 400. A value longer than 256 characters is therefore rejected with HTTP 400 / ValidationException. The message is one of the AWS constraint-validation messages that are BUILT FROM THE REQUEST, so its construction rule is a requirement: "1 validation error detected: Value '<the value exactly as submitted>' at '<memberName>' failed to satisfy constraint: Member must have length less than or equal to <max>". The recorded response shows that rule applied with the submitted value reproduced in full (a 300-character value), the member rendered as 'trajectoryId' and the bound 256.

Emulator: the status (400) and the error shape/code are correct, and the surrounding message template is correct, but the value interpolated into the message is CUT OFF at the constraint bound and replaced with an elision marker of the form "..." followed by the original byte count in brackets. The replayed step differs only in $.Message (verdict: mismatch, expected 400, actual 400): where AWS echoed the whole submitted value, the emulator echoed a prefix of it plus "... [300 bytes]".

Why: callers and SDK test suites parse this message to recover WHICH value was rejected — it is the only place the server reports the offending input back, and client-side assertions, log-based triage and regression fixtures compare the echoed value against what they sent. Because the emulator substitutes a shortened, annotated form, any client that round-trips or asserts on the rejected value sees a value it never submitted, and the byte-count marker is wire text AWS never emits. The defect is systematic in the shared constraint-message builder, so it will also corrupt the message for every other over-long string member of this service, not only TrajectoryId.

Fix: belongs in the input validation / error-message construction path for SampleWithResponseStream — specifically the shared helper that formats the "1 validation error detected: ..." text for @length violations. It must interpolate the submitted value unmodified rather than clamping it to the constraint maximum and appending a size annotation; the clamping/elision logic should be removed from message rendering (it is a presentation concern that does not belong on the wire). The status code and ValidationException classification already match and need no change.

Traces: sagemakerjobruntime-validation-errors#task#2 (SampleWithResponseStream, over-long X-Amzn-SageMaker-Trajectory-Id header).

## Evidence
Fixture sagemakerjobruntime-validation-errors, step task#2, operation SampleWithResponseStream, verdict mismatch.
Expected status: 400. Actual status: 400 (status and error shape agree).
Diverging key: $.Message (value_mismatch).
Expected (recorded AWS), rule-conformant — the full submitted 300-character value is echoed: "1 validation error detected: Value 'tttt...<all 300 characters>...tttt' at 'trajectoryId' failed to satisfy constraint: Member must have length less than or equal to 256".
Actual (emulator) — the echoed value is cut off and annotated: "1 validation error detected: Value 'tttt...<prefix only>... [300 bytes]' at 'trajectoryId' failed to satisfy constraint: Member must have length less than or equal to 256".
Model basis: SampleWithResponseStream.TrajectoryId -> target TrajectoryId, @httpHeader("X-Amzn-SageMaker-Trajectory-Id"), required, @length(min:1, max:256); operation error ValidationException @httpError(400).
The divergence is confined to the interpolated value: template text, member name ('trajectoryId') and the stated bound (256) all match.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture sagemakerjobruntime-validation-errors, step task#2 (replay_wire_corpus reports it as the sole divergence; score 85.7%, 6/7 steps passing).
2. Equivalently, by hand: POST /sample-with-response-stream to the sagemakerjobruntime endpoint with header X-Amzn-SageMaker-Job-Arn set to a valid job ARN, a non-empty body, and header X-Amzn-SageMaker-Trajectory-Id set to a 300-character string (any value longer than the modeled @length max of 256).
3. Observe the response: HTTP 400 with a ValidationException whose Message is "1 validation error detected: Value '<value>' at 'trajectoryId' failed to satisfy constraint: Member must have length less than or equal to 256".
4. Compare the value interpolated between the single quotes with the 300-character string that was sent. Expected: the two are byte-identical. Actual: the emulator emits only a leading portion of it followed by an elision marker and the byte count ("... [300 bytes]").
5. Confirm the fix by re-replaying sagemakerjobruntime-validation-errors#task#2 and checking that $.Message no longer diverges.
