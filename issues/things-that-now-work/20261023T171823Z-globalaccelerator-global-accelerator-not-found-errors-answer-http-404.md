Issue-Source: trace-sweep-divergence
Issue-Trace: globalaccelerator-accelerator-lifecycle
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: globalaccelerator-accelerator-lifecycle globalaccelerator-endpoint-group-lifecycle globalaccelerator-listener-lifecycle
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator|DescribeAccelerator|4xx->4xx|status
Issue-Key: globalaccelerator|DescribeEndpointGroup|4xx->4xx|$.Message
Issue-Key: globalaccelerator|DescribeEndpointGroup|4xx->4xx|status
Issue-Key: globalaccelerator|CreateListener|4xx->4xx|$.Message
Issue-Key: globalaccelerator|CreateListener|4xx->4xx|status
Issue-Key: globalaccelerator|DescribeListener|4xx->4xx|status
Issue-Commit: 326b33f63f33ac22c419b7169b10d47cfd8c011f

# Global Accelerator not-found errors answer HTTP 404 instead of recorded 400

> _Note: this issue was identified against the code base at commit `326b33f63f33ac22c419b7169b10d47cfd8c011f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccelerator

## Description
Global Accelerator is an awsJson1_1 service, so the HTTP status of an error response is part of the wire contract. Across four operations the emulator answers a reference to a resource that does not exist with HTTP 404, while every recorded AWS response in the corpus for the same condition is HTTP 400. The modelled error shape and error code themselves are correct in each case (no error-code divergence was reported); only the HTTP status line differs, and it differs the same way everywhere, which points at one shared status-mapping table for the *NotFoundException family rather than four independent handler bugs.

IMPORTANT MODEL/RECORDING CONFLICT a fixer must be aware of: the on-disk Smithy model declares @httpError 404 on these shapes (smithy_query shape AcceleratorNotFoundException / ListenerNotFoundException / EndpointGroupNotFoundException -> errorKind client, httpError 404; raw model carries "smithy.api#error":"client","smithy.api#httpError":404). The recorded wire corpus — which is what clients actually observe and what this corpus replays — shows 400 for all four. The recording is the authority for the emulated wire behavior here, so the status to emit is 400; the modelled 404 trait is the reason the emulator is currently wrong, so the fix belongs in the error-to-status mapping for this service and must not be driven straight off the trait.

Cause 1 of 4 — DescribeAccelerator on a deleted/unknown accelerator ARN. Fixture globalaccelerator-accelerator-lifecycle, step task#8.
AWS: recorded behavior only (and in conflict with the model trait, see above): AcceleratorNotFoundException returned with HTTP status 400. Smithy: DescribeAccelerator errors = AcceleratorNotFoundException (client), InvalidArgumentException (client, httpError 400), InternalServiceErrorException (500).
Emulator: HTTP 404 (detail: "status: expected 400, got 404").
Why: an SDK/CLI client keys retry and error classification off the status plus error code. A 404 on a JSON-protocol AWS endpoint is commonly treated by generic HTTP middleware, proxies and CDK/Terraform-style pollers as "endpoint or route missing" rather than "resource missing", so existence probes (describe-then-create, waiters that tolerate AcceleratorNotFound) take the wrong branch or surface a transport-level failure instead of a modelled exception.
Fix: the error-to-HTTP-status mapping used when serializing AcceleratorNotFoundException from the DescribeAccelerator handler.

Cause 2 of 4 — DescribeEndpointGroup on a deleted endpoint group. Fixture globalaccelerator-endpoint-group-lifecycle, step task#12.
AWS: recorded behavior only: EndpointGroupNotFoundException with HTTP status 400. Smithy: DescribeEndpointGroup errors = EndpointGroupNotFoundException (client), InvalidArgumentException (400), InternalServiceErrorException (500).
Emulator: HTTP 404. The same step also reports a Message text difference (expected 'EndpointGroup <arn> does not exist.', got 'Endpoint group <arn> not found.'); AWS does not document that string and it is not derived from a modelled constraint, so the recorded text is quoted here only as an example and is NOT part of this issue — the status is.
Why: same client misclassification as Cause 1, on the endpoint-group existence probe used by delete/recreate flows.
Fix: the error-to-HTTP-status mapping for EndpointGroupNotFoundException in the DescribeEndpointGroup handler.

Cause 3 of 4 — CreateListener against an accelerator ARN that does not exist. Fixture globalaccelerator-listener-lifecycle, step task#9.
AWS: recorded behavior only: AcceleratorNotFoundException with HTTP status 400. Smithy: CreateListener errors = AcceleratorNotFoundException (client), InvalidArgumentException (400), InvalidPortRangeException (400), LimitExceededException (403), InternalServiceErrorException (500).
Emulator: HTTP 404. The accompanying Message difference (expected 'Could not find accelerator.', got 'Accelerator <arn> not found.') is an example only, for the same reason as Cause 2.
Why: a create call that references a missing parent returns a status clients read as a routing failure, so provisioning tools report an endpoint problem instead of the actionable AcceleratorNotFound.
Fix: the error-to-HTTP-status mapping for AcceleratorNotFoundException in the CreateListener handler's parent-resource lookup.

Cause 4 of 4 — DescribeListener on a deleted listener ARN. Fixture globalaccelerator-listener-lifecycle, step task#12.
AWS: recorded behavior only: ListenerNotFoundException with HTTP status 400. Smithy: DescribeListener errors = ListenerNotFoundException (client), InvalidArgumentException (400), InternalServiceErrorException (500).
Emulator: HTTP 404 (detail: "status: expected 400, got 404").
Why: same client misclassification as Cause 1, on listener existence probes.
Fix: the error-to-HTTP-status mapping for ListenerNotFoundException in the DescribeListener handler.

Traces: globalaccelerator-accelerator-lifecycle#task#8, globalaccelerator-endpoint-group-lifecycle#task#12, globalaccelerator-listener-lifecycle#task#9, globalaccelerator-listener-lifecycle#task#12.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

1) globalaccelerator-accelerator-lifecycle#task#8 DescribeAccelerator — expected_status 400, actual_status 404. detail: "status: expected 400, got 404". divergence_key service=globalaccelerator|DescribeAccelerator|status=400->404|keys=status
2) globalaccelerator-endpoint-group-lifecycle#task#12 DescribeEndpointGroup — expected_status 400, actual_status 404. detail: "status: expected 400, got 404" plus "$.Message: value_mismatch (expected 'EndpointGroup arn:aws:globalaccelerator::000000000000:accelerator/896a5cf7-.../listener/2f9a4bf1/endpoint-group/48d4557a does not exist.', got 'Endpoint group ... not found.')" — message cited as example only.
3) globalaccelerator-listener-lifecycle#task#9 CreateListener — expected_status 400, actual_status 404. detail: "status: expected 400, got 404" plus "$.Message: value_mismatch (expected 'Could not find accelerator.', got 'Accelerator arn:aws:globalaccelerator::000000000000:accelerator/00000000-0000-0000-0000-000000000000 not found.')" — message cited as example only.
4) globalaccelerator-listener-lifecycle#task#12 DescribeListener — expected_status 400, actual_status 404. detail: "status: expected 400, got 404".

Model evidence (smithy_query, services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json): AcceleratorNotFoundException / ListenerNotFoundException / EndpointGroupNotFoundException each errorKind=client, httpError=404; InvalidArgumentException httpError=400. Recorded AWS responses in all four steps are 400, so the emulator's status mapping must follow the recording, not the trait.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus: replay_wire_corpus() for trace set simparity-wire-aws-v1.
2. Cause 1: run fixture globalaccelerator-accelerator-lifecycle to step task#8 — CreateAccelerator, DeleteAccelerator, then DescribeAccelerator with the deleted accelerator ARN. Observe HTTP 404; the recording is HTTP 400 with AcceleratorNotFoundException.
3. Cause 2: run fixture globalaccelerator-endpoint-group-lifecycle to step task#12 — DescribeEndpointGroup on the endpoint group deleted earlier in the fixture. Observe HTTP 404; recording is 400 with EndpointGroupNotFoundException.
4. Cause 3: run fixture globalaccelerator-listener-lifecycle to step task#9 — CreateListener with AcceleratorArn arn:aws:globalaccelerator::000000000000:accelerator/00000000-0000-0000-0000-000000000000. Observe HTTP 404; recording is 400 with AcceleratorNotFoundException.
5. Cause 4: run fixture globalaccelerator-listener-lifecycle to step task#12 — DescribeListener on the deleted listener ARN. Observe HTTP 404; recording is 400 with ListenerNotFoundException.
6. All four steps must replay clean (status 400, same error code) before the issue is closed.
