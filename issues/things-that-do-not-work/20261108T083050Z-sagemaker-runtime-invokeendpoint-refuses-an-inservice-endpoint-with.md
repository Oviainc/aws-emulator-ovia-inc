Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-runtime-invoke-endpoint-live-inference-roundtrip
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-runtime-invoke-endpoint-live-inference-roundtrip
Issue-Shape-Id: com.amazonaws.sagemakerruntime#AmazonSageMakerRuntime
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$.model_id
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$.predictions
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$.received_accept
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$.received_content_type
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$.received_instances
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$.sum
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|$headers.x-amzn-invoked-production-variant
Issue-Key: sagemaker-runtime|InvokeEndpoint|2xx->4xx|status
Issue-Commit: fc478c900f96ca5c80ec03cf91413f5c0fb5bcda

# sagemaker-runtime InvokeEndpoint refuses an InService endpoint with 424 ModelError instead of serving a 200 inference response

> _Note: this issue was identified against the code base at commit `fc478c900f96ca5c80ec03cf91413f5c0fb5bcda`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeEndpoint

## Description
One cause (Cause 1 of 1): the sagemaker-runtime data plane serves no inference at all — every InvokeEndpoint against an endpoint the emulator itself reports InService short-circuits to the modeled ModelError refusal instead of returning the invocation's 200 response.

AWS: Smithy com.amazonaws.sagemakerruntime#InvokeEndpoint is bound http {method: POST, uri: /endpoints/{EndpointName}/invocations, code: 200}. For an endpoint whose DescribeEndpoint reports EndpointStatus=InService and whose requested TargetVariant exists, the operation returns HTTP 200 whose InvokeEndpointOutput carries the container's response as the @httpPayload Body (required, BodyBlob) with the Content-Type the container produced, plus the response header x-Amzn-Invoked-Production-Variant (InvokeEndpointOutput$InvokedProductionVariant) naming the ProductionVariant that actually handled the request. ModelError is a @error("client") with httpError 424 and is documented as "Model (owned by the customer in the container) returned 4xx or 5xx error code" — i.e. it reports a failure ORIGINATING IN the customer's container, and it is not a legitimate answer for a healthy variant on an InService endpoint. The response Body's content is whatever the hosted container emits, so its shape is recorded behavior only; the recorded response for this fixture is a JSON document echoing the invocation (model_id, received_content_type, received_accept, received_instances, and derived prediction fields predictions/sum) with the invoked-variant header set to the endpoint's single production variant name.

Emulator: the same step answers HTTP 424 with the ModelError body and no payload at all — the replay reports `status: expected 200, got 424` and every asserted response member missing (`$.model_id`, `$.predictions`, `$.received_accept`, `$.received_content_type`, `$.received_instances`, `$.sum` all `got None`), together with the missing response header `x-amzn-invoked-production-variant` (expected the endpoint's production-variant name). The refusal's message text is emulator-authored prose about the emulator having no model-inference container, naming only the endpoint from the request; AWS never emits that sentence for this error class.

Why: the runtime data plane is a dead end for every client. A caller that deploys a model and invokes it cannot get a response body, cannot read which production variant served the request (the header A/B and canary logic branches on), and cannot tell a genuine container-side 4xx/5xx — the real meaning of ModelError, which SDK users retry, alert and roll back on — from "this backend serves nothing". Any downstream behaviour keyed on a successful invocation (data capture, inference-id correlation, session continuation via NewSessionId) is unreachable, and multi-step flows that invoke after a successful deploy abort at the first invocation.

Fix: the InvokeEndpoint handler of the sagemaker-runtime data plane and the endpoint/variant state it reads. An endpoint in InService with a resolvable production variant must serve a deterministic 200 response derived from the request (echoing the received Content-Type/Accept and the decoded Body) rather than returning ModelError, and the serializer must set the x-Amzn-Invoked-Production-Variant response header to the variant that handled the call. ModelError/424 must be reserved for a genuine container-side failure.

Related open issues that do NOT cover this step: 20261029T203709Z-s3-sagemaker-training-jobs-never-execute-no-model.md describes the same 424 ModelError symptom but is a BEHAVIORAL issue keyed on s3__sagemaker check ids (Issue-Key sagemaker-runtime|InvokeEndpoint|ModelError) and carries no wire replay key, so this fixture step is unowned; 20261029T233857Z-sagemaker-runtime-drops-errorcode-message-from-refusal-bodies.md and 20261101T041426Z-sagemaker-runtime-constraint-violation-refusal-messages-strip-the.md own the two other divergences in this fixture family (refusal-body shape and @pattern anchor stripping) and not this one.

Traces: sagemaker-runtime-invoke-endpoint-live-inference-roundtrip#task#9.

## Evidence
Fixture sagemaker-runtime-invoke-endpoint-live-inference-roundtrip, step task#9 (POST /endpoints/{EndpointName}/invocations after the endpoint reached InService):

Expected (recorded AWS response): HTTP 200; response header x-amzn-invoked-production-variant = the endpoint's single production-variant name (recorded value 'sim2realprobevariant' — a generated name, not a requirement); body a JSON document with model_id (recorded 'sim2real-constant-v1'), received_content_type = the request's Content-Type ('application/json'), received_accept = the request's Accept ('application/json'), received_instances = the instances decoded from the request Body ([[1.0, 2.0, 3.0]]), predictions ([0.25, 0.75]) and sum (6.0) derived from those instances.

Actual (emulator): HTTP 424 with the ModelError refusal and no response payload — replay detail: "status: expected 200, got 424; $.model_id: missing (expected 'sim2real-constant-v1', got None); $.predictions: missing (expected [0.25, 0.75], got None); $.received_accept: missing (expected 'application/json', got None); $.received_content_type: missing (expected 'application/json', got None); $.received_instances: missing (expected [[1.0, 2.0, 3.0]], got None); $.sum: missing (expected 6.0, got None); $headers.x-amzn-invoked-production-variant: missing (expected 'sim2realprobevariant', got None)". Status 424 and error code ModelError are the diverging requirement values; the refusal message is emulator-authored (fixed text about the emulator running no model inference container, with only the endpoint name taken from the request), where AWS's ModelError text reports the status and body the customer's container returned.

Model evidence: Smithy com.amazonaws.sagemakerruntime#InvokeEndpoint http {POST, /endpoints/{EndpointName}/invocations, code 200}; InvokeEndpointOutput$Body is the required @httpPayload; InvokeEndpointOutput$InvokedProductionVariant is bound to response header x-Amzn-Invoked-Production-Variant; ModelError is errorKind client with httpError 424, documented "Model (owned by the customer in the container) returned 4xx or 5xx error code."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture sagemaker-runtime-invoke-endpoint-live-inference-roundtrip.
2. The fixture's earlier steps create a model and an endpoint config with a single production variant, create the endpoint and wait until DescribeEndpoint reports EndpointStatus=InService (those steps pass).
3. At step task#9, call sagemaker-runtime InvokeEndpoint with EndpointName = that endpoint, Content-Type: application/json, Accept: application/json and a JSON Body carrying one instance row.
4. Observe HTTP 424 with error code ModelError and an emulator-authored no-inference-container message, instead of HTTP 200 whose body echoes the received content type/accept/instances with the derived prediction fields and whose x-Amzn-Invoked-Production-Variant header names the endpoint's production variant.
