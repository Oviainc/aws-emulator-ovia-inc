Issue-Source: trace-sweep-divergence
Issue-Trace: synthetics-canary-missing-resource-errors
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: synthetics-canary-missing-resource-errors
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|GetCanary|4xx->4xx|$.Message
Issue-Key: synthetics|DeleteCanary|4xx->4xx|$.Message
Issue-Key: synthetics|StartCanary|4xx->4xx|$.Message
Issue-Key: synthetics|StopCanary|4xx->4xx|$.Message
Issue-Key: synthetics|GetCanaryRuns|4xx->4xx|$.Message
Issue-Commit: dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4

# Synthetics not-found errors echo the requested canary name instead of AWS's fixed message

> _Note: this issue was identified against the code base at commit `dcb14db06d6eb8eb19e965c6ac371e7edd3d0be4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCanary

## Description
Every synthetics operation that resolves a canary by name builds its ResourceNotFoundException message by interpolating the requested name, whereas the recorded service answers with one fixed sentence that never names the resource. Status (404) and error type match in all five cases; only the Message member diverges. 5 causes, all on fixture synthetics-canary-missing-resource-errors.

AWS: recorded behavior only — the Smithy model says of ResourceNotFoundException only "One of the specified resources was not found." with a single Message member (target ErrorMessage), and documents no message text. The recording establishes the rule as an invariant rather than as one string: five different operations (GetCanary, DeleteCanary, StartCanary, StopCanary, GetCanaryRuns), each called with a canary name that does not exist, all answer HTTP 404 with the identical constant Message 'Canary not found.' — the requested name appears nowhere in the response. So the message is a fixed, request-independent sentence; it does not echo the lookup key.
Hypothesis: the constant form is deliberate, since CreateCanary's own documentation warns that a canary name is included in outbound calls and should not carry proprietary information.

Emulator: each handler formats the name into the message, e.g. src/ops/get_canary.rs line 176: message: Some(format!("Canary {} not found", input.name)). Replay reports, for all five operations, $.Message: value_mismatch (expected 'Canary not found.', got 'Canary sim2real-probe-synthetics-canary-missing-76a48b91 not found'). The HTTP status is 404 as expected and no other field diverges.

Why: the emulator's message is request-derived where AWS's is constant, so a client cannot rely on either form: a caller that matches or asserts on the not-found message sees a different sentence per request, and one that reflects the error back to a user is handed the resource identifier that the real service withholds. The divergence is the message-construction rule, not the particular recorded sentence.

Fix: the not-found error construction in each of these five operation handlers (ideally hoisted into one shared helper in the service's state/error module alongside STORE_UNAVAILABLE) must emit the fixed, name-free ResourceNotFoundException message rather than formatting the input name into it.

Cause 1 of 5: GetCanary — fixture synthetics-canary-missing-resource-errors, step task#1.
Cause 2 of 5: DeleteCanary — fixture synthetics-canary-missing-resource-errors, step task#2.
Cause 3 of 5: StartCanary — fixture synthetics-canary-missing-resource-errors, step task#4.
Cause 4 of 5: StopCanary — fixture synthetics-canary-missing-resource-errors, step task#5.
Cause 5 of 5: GetCanaryRuns — fixture synthetics-canary-missing-resource-errors, step task#7.

Traces: the five steps named above. No other synthetics issue is open.

## Evidence
Replay divergences (all fixture synthetics-canary-missing-resource-errors, verdict mismatch, expected_status 404 / actual_status 404):
- task#1 GetCanary: $.Message: value_mismatch (expected 'Canary not found.', got 'Canary sim2real-probe-synthetics-canary-missing-76a48b91 not found')
- task#2 DeleteCanary: same detail
- task#4 StartCanary: same detail
- task#5 StopCanary: same detail
- task#7 GetCanaryRuns: same detail

Expected: 404 ResourceNotFoundException whose Message is the fixed, request-independent sentence recorded for all five operations ('Canary not found.'). Actual: 404 ResourceNotFoundException whose Message interpolates the requested canary name.

Model: ResourceNotFoundException = structure {Message: ErrorMessage}, errorKind client, httpError 404; modeled on DeleteCanary/StartCanary/StopCanary/GetCanaryRuns (GetCanary's errors list omits it although both AWS and the emulator answer 404 there). Code: services/service-com-amazonaws-synthetics-synthetics/src/ops/get_canary.rs lines 170-179 (format!("Canary {} not found", input.name)); the sibling handlers for DeleteCanary, StartCanary, StopCanary and GetCanaryRuns build the same message.

## Steps to Reproduce
1. git_clone(); 2. replay_wire_corpus(); 3. Inspect fixture synthetics-canary-missing-resource-errors, steps task#1, task#2, task#4, task#5, task#7.
Equivalently, against a canary name that does not exist (e.g. 'no-such-canary') on the emulator's synthetics endpoint:
- GET /canary/no-such-canary
- DELETE /canary/no-such-canary
- POST /canary/no-such-canary/start
- POST /canary/no-such-canary/stop
- POST /canary/no-such-canary/runs
Each returns 404 with Message 'Canary no-such-canary not found'; the recorded service returns 404 with a constant Message that does not contain the requested name.
