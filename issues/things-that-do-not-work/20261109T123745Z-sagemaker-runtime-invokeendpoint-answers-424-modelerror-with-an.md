Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-train-host-and-invoke-xgboost-model
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-train-host-and-invoke-xgboost-model
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$headers.content-type
Issue-Related: 20261108T083050Z-sagemaker-runtime-invokeendpoint-refuses-an-inservice-endpoint-with.md
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# sagemaker-runtime InvokeEndpoint answers 424 ModelError with an emulator-authored message instead of a 200 inference response

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpoint

## Description
1 cause, in the InvokeEndpoint handler's inference path.

Cause 1 of 1 - InvokeEndpoint refuses an InService endpoint with HTTP 424 ModelError and a message that exists nowhere in AWS, instead of returning the endpoint's inference output.
AWS: Smithy: InvokeEndpoint is @http(method POST, uri /endpoints/{EndpointName}/invocations, code 200); its output members are Body (@httpPayload, the container's response bytes), ContentType (@httpHeader Content-Type) and InvokedProductionVariant (@httpHeader x-Amzn-Invoked-Production-Variant), plus the optional CustomAttributes/NewSessionId/ClosedSessionId headers. Invoking an endpoint whose EndpointStatus is InService returns HTTP 200: the body is whatever the model container wrote, Content-Type is the content type the container responded with (for the recorded xgboost container, text/csv; charset=utf-8), and x-Amzn-Invoked-Production-Variant names the production variant that served the request - here the endpoint config's single variant, whose name is fixed text taken from the endpoint configuration ('AllTraffic'). ModelError IS modeled on this operation as a 424 client error, but it is the class for a container that ran and returned an error or an unparseable response; it is not a status AWS uses for an InService endpoint that is otherwise healthy.
Emulator: answers HTTP 424 with Content-Type application/json and the JSON body {"__type": "ModelError", "Message": "This emulator runs no model inference container, so endpoint <endpoint-name> returned no inference for this request."}, and omits the x-amzn-invoked-production-variant header entirely. Every part of that message after the endpoint name is emulator-authored prose describing the emulator's own limitation; no AWS error message mentions an emulator or a missing container runtime, so the text is a defect in itself independently of the status. (In the recorded step the variable part is the endpoint name taken from the request's {EndpointName} label; the rest of the sentence is fixed emulator text.)
Why: this is the terminal step of every hosted-inference flow - a client trains, creates a model, deploys an endpoint, waits for InService, then invokes. A 424 at that point tells the client its MODEL is broken, so SDK retry logic does not retry (424 is a client error) and application code routes into its model-failure branch, reports a bad model, or rolls the deployment back. No caller can obtain an inference result or verify that a deployment serves traffic, and no caller can distinguish a genuinely failing container from this blanket refusal. The missing x-Amzn-Invoked-Production-Variant header additionally breaks any client that attributes a response to a variant (A/B traffic splitting, shadow-variant comparison).
Fix: the InvokeEndpoint handler's inference path (sagemaker-runtime data plane) - an InService endpoint must produce a 200 response carrying a body for the invoked variant, the container's Content-Type, and the x-Amzn-Invoked-Production-Variant header naming the variant selected from the endpoint config (honouring X-Amzn-SageMaker-Target-Variant when supplied); 424 ModelError must be reserved for a container-level failure.
Traces: sagemaker-train-host-and-invoke-xgboost-model#task#12.

Related open issues, none of which covers this: 20261108T083050Z-sagemaker-runtime-invokeendpoint-refuses-an-inservice-endpoint-with.md describes the same refusal but the replay reports this cluster as unowned, i.e. that issue carries no replay key binding it to sagemaker-train-host-and-invoke-xgboost-model#task#12, so a fixer re-replaying it would never exercise this step; 20261029T233857Z-sagemaker-runtime-drops-errorcode-message-from-refusal-bodies.md and 20261031T024432Z / 20261101T041426Z are about refusal-body shape and @pattern anchors on the ValidationError path, not about the inference path returning 424 for a healthy endpoint.

## Evidence
sagemaker-train-host-and-invoke-xgboost-model#task#12 InvokeEndpoint, divergence_key service=sagemaker-runtime|InvokeEndpoint|status=200->424|keys=$,$headers.content-type,$headers.x-amzn-invoked-production-variant,status:
  status: expected 200, got 424
  $: expected the container's inference payload ('16.488086700439453,2.494762659072876' in this run - a per-run model output, not a required value; the requirement is a 200 body for the invoked variant), got {'__type': 'ModelError', 'Message': 'This emulator runs no model inference container, so endpoint sim2real-probe-sagemaker-c2e04ab4 returned no inference for this request.'}
  $headers.x-amzn-invoked-production-variant: missing (expected 'AllTraffic', got None) - 'AllTraffic' is the variant name from the endpoint configuration, i.e. a value from state, and the header itself is required on a 200
  $headers.content-type: expected 'text/csv; charset=utf-8' (the container's response content type), got 'application/json'
In the emulator's Message, 'This emulator runs no model inference container, so endpoint ' and ' returned no inference for this request.' are fixed emulator-authored text and only the endpoint name is taken from the request; AWS words no error this way.

## Steps to Reproduce
1. Replay fixture sagemaker-train-host-and-invoke-xgboost-model up to task#12: the fixture creates a training job, a model, an endpoint config and an endpoint, and waits for the endpoint to report InService.
2. POST to /endpoints/{EndpointName}/invocations with the CSV payload (Content-Type text/csv).
3. Observe HTTP 424 with Content-Type application/json and body {"__type":"ModelError","Message":"This emulator runs no model inference container, ..."} and no x-amzn-invoked-production-variant response header, where the recording has HTTP 200, Content-Type 'text/csv; charset=utf-8', x-amzn-invoked-production-variant 'AllTraffic' and an inference body.
