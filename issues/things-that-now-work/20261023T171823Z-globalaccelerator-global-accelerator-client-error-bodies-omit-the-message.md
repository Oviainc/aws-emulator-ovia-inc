Issue-Source: trace-sweep-divergence
Issue-Trace: globalaccelerator-accelerator-tagging
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: globalaccelerator-accelerator-tagging globalaccelerator-endpoint-group-lifecycle
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator|TagResource|4xx->4xx|$.Message
Issue-Key: globalaccelerator|UpdateEndpointGroup|4xx->4xx|$.Message
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Global Accelerator client-error bodies omit the Message member entirely

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two Global Accelerator client errors come back with the right HTTP status but a body that has no Message member at all (got None, not a differently worded string). Every Global Accelerator error shape in the model declares a Message member (target ErrorMessage), and the awsJson1_1 error body is where SDKs read it from, so an error serialized without it is structurally short of the modelled shape. Because both occurrences are on validation rejections produced by different handlers, the shared fault is the error serializer/constructor path that builds InvalidArgumentException-family bodies without populating Message.

Note: this issue is specifically about the ABSENCE of the member. Steps where the emulator does emit a Message that merely reads differently from the recording are deliberately not bundled here.

Cause 1 of 2 — TagResource rejects an invalid tag input with a Message-less body. Fixture globalaccelerator-accelerator-tagging, step task#10.
AWS: Smithy: TagResource(ResourceArn required, length 1-1011; Tags required) errors = InvalidArgumentException (client, httpError 400), AcceleratorNotFoundException, InternalServiceErrorException. InvalidArgumentException is defined as structure { Message: ErrorMessage } with @error("client"). The recorded response is HTTP 400 carrying Message 'Invalid parameter.' — AWS does not document that exact wording, so the requirement proven here is that the error body carries a populated Message member describing the rejected parameter; the recorded string is an example.
Emulator: HTTP 400 with the Message member missing from the JSON body (detail: "$.Message: missing (expected 'Invalid parameter.', got None)").
Why: an SDK surfaces the error with an empty message, so CLI output and logs read like "InvalidArgumentException: " with no reason. Client code that branches on, or operators that triage from, the error text get nothing to act on, and anything asserting a non-empty message fails against the emulator while passing against AWS.
Fix: the error serializer / exception constructor used by the TagResource handler's input validation — it must populate the Message member of the error structure it emits.

Cause 2 of 2 — UpdateEndpointGroup range violation rejected with a Message-less body. Fixture globalaccelerator-endpoint-group-lifecycle, step task#8.
AWS: Smithy: UpdateEndpointGroup declares range-constrained numeric members (TrafficDialPercentage float range 0-100, HealthCheckPort 1-65535, HealthCheckIntervalSeconds 10-30, ThresholdCount 1-10) and errors InvalidArgumentException (client, httpError 400), EndpointGroupNotFoundException, AccessDeniedException, LimitExceededException, InternalServiceErrorException. The recorded response is HTTP 400 with Message "1 validation error detected: Value '150.0' at 'trafficRetentionPercentage' failed to satisfy constraint: Member must have value less than or equal to 100". Here the message rule IS known — it is built from the request: the offending value, the member name, and the violated upper bound. That rule is the requirement, not the recorded text.
Emulator: HTTP 400 with no Message member at all (detail: "$.Message: missing (expected \"1 validation error detected: ...\", got None)"). The constraint itself is detected (status is right); only the explanatory member is absent.
Why: a caller that sent an out-of-range percentage is told only "400 InvalidArgumentException" with no indication of which member or bound was violated, so neither a human nor automated remediation can tell which of the several range-constrained members of UpdateEndpointGroup was at fault.
Fix: the validation layer of the UpdateEndpointGroup handler — emit the constraint violation with a Message built from the rejected value, the member name and the violated bound, as the serializer in Cause 1 must also do.

Traces: globalaccelerator-accelerator-tagging#task#10, globalaccelerator-endpoint-group-lifecycle#task#8 (cluster also covers task#9).

## Evidence
Replay of trace set simparity-wire-aws-v1:

1) globalaccelerator-accelerator-tagging#task#10 TagResource — expected_status 400, actual_status 400, detail: "$.Message: missing (expected 'Invalid parameter.', got None)". divergence_key service=globalaccelerator|TagResource|status=400->400|keys=$.Message
2) globalaccelerator-endpoint-group-lifecycle#task#8 UpdateEndpointGroup — expected_status 400, actual_status 400, detail: "$.Message: missing (expected \"1 validation error detected: Value '150.0' at 'trafficRetentionPercentage' failed to satisfy constraint: Member must have value less than or equal to 100\", got None)". affected_count 2 (also globalaccelerator-endpoint-group-lifecycle#task#9). divergence_key service=globalaccelerator|UpdateEndpointGroup|status=400->400|keys=$.Message

In both cases the expected-vs-actual difference is "missing / got None" — the member is absent from the JSON error body, not merely worded differently.

Model evidence (smithy_query on services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json): InvalidArgumentException = structure { Message: ErrorMessage }, errorKind client, httpError 400. UpdateEndpointGroup input TrafficDialPercentage float range {min 0, max 100}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus() for trace set simparity-wire-aws-v1.
2. Cause 1: run fixture globalaccelerator-accelerator-tagging to step task#10 — TagResource with the invalid tag input recorded there. Response is HTTP 400 but the JSON body contains no "Message" key; AWS returns 400 with a populated Message (recorded example: 'Invalid parameter.').
3. Cause 2: run fixture globalaccelerator-endpoint-group-lifecycle to step task#8 — UpdateEndpointGroup with a traffic percentage of 150.0 (above the modelled max of 100). Response is HTTP 400 with no "Message" key; AWS returns 400 and a Message naming the value, the member and the violated bound. Step task#9 of the same fixture is in the same cluster.
4. Both steps must replay with a non-empty Message in the error body before closing.
