Issue-Source: trace-sweep-divergence
Issue-Trace: globalaccelerator-custom-routing-separation
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: globalaccelerator-custom-routing-separation
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator|DescribeAccelerator|4xx->4xx|$.Message
Issue-Commit: e2a7b75abf70ab47f92f0d33522cc824ac8688ce

# globalaccelerator DescribeAccelerator on a custom routing accelerator returns a not-found message instead of the custom-routing API separation message

> _Note: this issue was identified against the code base at commit `e2a7b75abf70ab47f92f0d33522cc824ac8688ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAccelerator

## Description
One net-new wire divergence in the globalaccelerator emulator: when DescribeAccelerator (the standard-accelerator API) is called with the ARN of a CUSTOM ROUTING accelerator, the emulator reports the resource as simply absent instead of reporting that the resource exists but belongs to the Custom Routing API family. The HTTP status (400) and the error code match the recording; only the error body's `Message` member diverges. Global Accelerator is an awsJson1_1 service whose errors carry a human-readable `Message`, which SDKs, the CLI and test assertions surface, so a wrong message is a wire defect even when status and code are right.

Cause 1 of 1: DescribeAccelerator addressed at a custom routing accelerator ARN.
AWS: Smithy (services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json) models DescribeAccelerator with a single required input member AcceleratorArn (GenericString, @length 0..255) and errors AcceleratorNotFoundException (httpError 404), InvalidArgumentException (httpError 400) and InternalServiceErrorException (500). Standard accelerators and custom routing accelerators are distinct resource types with distinct APIs (DescribeAccelerator vs DescribeCustomRoutingAccelerator, each modeled with its own output shape, Accelerator vs CustomRoutingAccelerator). The model does not state the wording, so the message is recorded behavior only: the recording shows HTTP 400 with the error code the emulator also returns, and Message of the form 'Custom Routing resources must use the Custom Routing APIs; got custom routing resource: <acceleratorArn>'. The fixed text is 'Custom Routing resources must use the Custom Routing APIs; got custom routing resource: '; the single variable part is the accelerator ARN taken verbatim from the request's AcceleratorArn member. The recorded run's ARN value (arn:aws:globalaccelerator::000000000000:accelerator/d757d551-9d75-42e4-b17f-533061f02814) is a per-run generated identifier filling that slot, not part of the requirement.
Emulator: returns HTTP 400 with Message of the form 'Accelerator <acceleratorArn> not found.' — i.e. it falls through to its generic accelerator-lookup-miss message, because its standard-accelerator lookup simply does not find custom routing accelerators in state and treats the ARN as unknown. It never distinguishes 'this ARN names no resource' from 'this ARN names a custom routing resource, which this API does not serve'.
Why: a client gets the wrong diagnosis for a recoverable mistake. The real failure is that the caller used the wrong API family for an accelerator that genuinely exists, and AWS's message tells it so and names the offending resource; the emulator tells it the accelerator does not exist. Error handling or retry/provisioning logic keyed on AWS's custom-routing separation message never fires, and callers that branch on 'not found' will go on to (re)create a resource that already exists or report a phantom deletion. Matching the custom-routing sentence against the emulator also fails outright.
Fix: DescribeAccelerator's handler / accelerator lookup in the globalaccelerator state layer — the standard-accelerator resolution path must consult custom routing accelerators too and, on a hit, raise the API-separation error with the service's fixed sentence interpolating only the request's AcceleratorArn, rather than returning the generic accelerator-not-found message.
Traces: globalaccelerator-custom-routing-separation#task#8.

Relation to already-open work: the open issue 20261029T171730Z-globalaccelerator-error-messages-diverge-from-aws-wording-across.md covers five other globalaccelerator $.Message divergences (CreateAccelerator, CreateEndpointGroup, UpdateEndpointGroup, DeleteListener, CreateListener). It does NOT cover this one — it carries no replay key for globalaccelerator-custom-routing-separation and its causes are all validation/conflict message templates, whereas this cause is a resource-resolution defect (custom routing resources are invisible to the standard-accelerator lookup), not merely wording.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false, 7 fixtures replayed, 96 steps, 45 passed, 37 mismatched, 6 divergence clusters). Five clusters are owned_by the open issue 20261029T171730Z-globalaccelerator-error-messages-diverge-from-aws-wording-across.md; this cluster has empty owned_by and empty stub_path.

Cluster: divergence_key 'service=globalaccelerator|DescribeAccelerator|status=400->400|keys=$.Message', verdict mismatch, service globalaccelerator, operation DescribeAccelerator, fixture_id globalaccelerator-custom-routing-separation, step_key task#8, affected_count 1.
  expected_status 400, actual_status 400 (status and error code agree; the sole diverging key is $.Message).
  Expected $.Message: 'Custom Routing resources must use the Custom Routing APIs; got custom routing resource: arn:aws:globalaccelerator::000000000000:accelerator/d757d551-9d75-42e4-b17f-533061f02814'
  Actual   $.Message: 'Accelerator arn:aws:globalaccelerator::000000000000:accelerator/d757d551-9d75-42e4-b17f-533061f02814 not found.'
  Fixed vs variable text: expected message's fixed part is 'Custom Routing resources must use the Custom Routing APIs; got custom routing resource: ' and its only variable part is the request's AcceleratorArn; the ARN/UUID shown is a per-run generated value and is not itself a requirement.

Model grounding (smithy_query, services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json): DescribeAccelerator input {AcceleratorArn: GenericString, required, @length 0..255}, output {Accelerator: Accelerator}, errors AcceleratorNotFoundException(404), InvalidArgumentException(400), InternalServiceErrorException(500). DescribeCustomRoutingAccelerator is a separate operation with the same input member but output {Accelerator: CustomRoutingAccelerator}, confirming the two resource families are modeled distinctly and that a custom routing ARN is a real resource rather than an unknown one.

## Steps to Reproduce
1. Clone the repository and boot the emulator built from that checkout.
2. Replay wire trace set simparity-wire-aws-v1.
3. Replay fixture globalaccelerator-custom-routing-separation up to step task#8: the fixture first creates a CUSTOM ROUTING accelerator (via the Custom Routing API), then calls DescribeAccelerator — the standard-accelerator API — with that custom routing accelerator's ARN.
4. Inspect the 400 error body. Expected: error code as recorded with Message 'Custom Routing resources must use the Custom Routing APIs; got custom routing resource: <the AcceleratorArn sent in the request>'. Actual: Message 'Accelerator <the AcceleratorArn sent in the request> not found.'
5. Confirm the resource does exist by calling DescribeCustomRoutingAccelerator with the same ARN — it succeeds, so 'not found' is the wrong diagnosis.
