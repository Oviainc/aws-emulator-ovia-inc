Issue-Source: trace-sweep-divergence
Issue-Trace: route53-traffic-policy-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53-traffic-policy-error-paths route53-traffic-policy-instance-guardrails route53-traffic-policy-versioning
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route53|DeleteTrafficPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|GetTrafficPolicyInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|DeleteTrafficPolicyInstance|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: route53|GetTrafficPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: a9b33a473a9762dcd2804e6444a9b0695dc27129

# Route 53 traffic-policy not-found refusals add an Error.Message element AWS omits

> _Note: this issue was identified against the code base at commit `a9b33a473a9762dcd2804e6444a9b0695dc27129`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTrafficPolicy

## Description
Route 53's traffic-policy and traffic-policy-instance not-found refusals carry an ErrorResponse whose Error element has Type and Code but NO Message child. The emulator synthesises a Message on all four of these paths, so every body carries an element AWS does not emit. 4 causes, one per operation.

Cause 1 of 4 — DeleteTrafficPolicy, unknown policy Id/Version.
AWS: Smithy: DeleteTrafficPolicy binds NoSuchTrafficPolicy with httpError 404. Recorded behavior only for the body shape: the refusal is HTTP 404 with ErrorResponse/Error carrying Type `Sender` and Code `NoSuchTrafficPolicy` and no Message element at all (the recording asserts Message absent).
Emulator: emits Error.Message 'The policy does not exist.' — a sentence taken from the error shape's documentation rather than from the wire.
Why: clients and SDK error mappers that treat the presence of Message as meaningful (logging it, matching on it, surfacing it to users) see a message AWS never sends, and any assertion that the field is absent fails.
Fix: the NoSuchTrafficPolicy serializer for DeleteTrafficPolicy — emit Type and Code only.
Traces: route53-traffic-policy-error-paths#task#2.

Cause 2 of 4 — GetTrafficPolicyInstance, unknown instance Id.
AWS: Smithy: GetTrafficPolicyInstance binds NoSuchTrafficPolicyInstance with httpError 404. Recorded behavior only: HTTP 404 with Error carrying Type and Code `NoSuchTrafficPolicyInstance` and no Message element.
Emulator: emits Error.Message 'No traffic policy instance exists with ID <id>', interpolating the requested instance id into a sentence AWS does not send on this path.
Why: same as cause 1; additionally the emulator leaks the requested id into a field that should not exist.
Fix: the NoSuchTrafficPolicyInstance serializer for GetTrafficPolicyInstance.
Traces: route53-traffic-policy-instance-guardrails#task#4.

Cause 3 of 4 — DeleteTrafficPolicyInstance, unknown instance Id.
AWS: Smithy: DeleteTrafficPolicyInstance binds NoSuchTrafficPolicyInstance with httpError 404. Recorded behavior only: HTTP 404, Error with Type and Code only, no Message.
Emulator: emits the same interpolated 'No traffic policy instance exists with ID <id>' Message.
Why: same as cause 2.
Fix: the NoSuchTrafficPolicyInstance serializer for DeleteTrafficPolicyInstance (shared with cause 2).
Traces: route53-traffic-policy-instance-guardrails#task#6.

Cause 4 of 4 — GetTrafficPolicy, unknown policy Id/Version.
AWS: Smithy: GetTrafficPolicy binds NoSuchTrafficPolicy with httpError 404. Recorded behavior only: HTTP 404, Error with Type and Code `NoSuchTrafficPolicy` only, no Message.
Emulator: emits Error.Message 'The policy does not exist.'
Why: same as cause 1.
Fix: the NoSuchTrafficPolicy serializer for GetTrafficPolicy (shared with cause 1).
Traces: route53-traffic-policy-versioning#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 — in all four clusters the HTTP status matched (404) and only the message element diverged:
- route53-traffic-policy-error-paths#task#2 DeleteTrafficPolicy: $.ErrorResponse.Error.Message unexpected_present (expected None, got 'The policy does not exist.').
- route53-traffic-policy-instance-guardrails#task#4 GetTrafficPolicyInstance: $.ErrorResponse.Error.Message unexpected_present (expected None, got 'No traffic policy instance exists with ID <id>').
- route53-traffic-policy-instance-guardrails#task#6 DeleteTrafficPolicyInstance: $.ErrorResponse.Error.Message unexpected_present (expected None, got 'No traffic policy instance exists with ID <id>').
- route53-traffic-policy-versioning#task#10 GetTrafficPolicy: $.ErrorResponse.Error.Message unexpected_present (expected None, got 'The policy does not exist.').

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus and inspect the four steps under Traces.
3. Individually: GET and DELETE /2013-04-01/trafficpolicy/<unknown-id>/1, and GET and DELETE /2013-04-01/trafficpolicyinstance/<unknown-id>.
4. Each 404 body contains ErrorResponse/Error/Message; AWS's recorded bodies contain only Type and Code.
