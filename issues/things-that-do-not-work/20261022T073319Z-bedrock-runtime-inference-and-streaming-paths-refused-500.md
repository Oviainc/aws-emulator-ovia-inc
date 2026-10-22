Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-runtime-converse-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-runtime-converse-error-paths bedrock-runtime-converse-error-paths-and-recovery bedrockruntime-converse-envelope-and-rejections bedrock-runtime-converse-stream-basic bedrockruntime-streaming-frame-order bedrock-runtime-invoke-model-error-paths bedrock-runtime-count-tokens-before-invoke bedrock-runtime-invoke-model-invalid-content-type bedrock-runtime-invoke-model-response-stream bedrock-runtime-invoke-model-with-response-stream-titan
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|Converse|4xx->5xx|$.message
Issue-Key: bedrock-runtime|Converse|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|Converse|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|Converse|4xx->5xx|status
Issue-Key: bedrock-runtime|Converse|2xx->5xx|$.metrics
Issue-Key: bedrock-runtime|Converse|2xx->5xx|$.output
Issue-Key: bedrock-runtime|Converse|2xx->5xx|$.stopReason
Issue-Key: bedrock-runtime|Converse|2xx->5xx|$.usage
Issue-Key: bedrock-runtime|Converse|2xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|Converse|2xx->5xx|status
Issue-Key: bedrock-runtime|ConverseStream|2xx->5xx|$.__wire_eventstream__
Issue-Key: bedrock-runtime|ConverseStream|2xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|ConverseStream|2xx->5xx|status
Issue-Key: bedrock-runtime|ConverseStream|4xx->5xx|$.message
Issue-Key: bedrock-runtime|ConverseStream|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|ConverseStream|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|ConverseStream|4xx->5xx|status
Issue-Key: bedrock-runtime|InvokeModel|4xx->5xx|$.message
Issue-Key: bedrock-runtime|InvokeModel|4xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|InvokeModel|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|InvokeModel|4xx->5xx|status
Issue-Key: bedrock-runtime|InvokeModel|2xx->5xx|$.output
Issue-Key: bedrock-runtime|InvokeModel|2xx->5xx|$.stopReason
Issue-Key: bedrock-runtime|InvokeModel|2xx->5xx|$.usage
Issue-Key: bedrock-runtime|InvokeModel|2xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|InvokeModel|2xx->5xx|status
Issue-Key: bedrock-runtime|InvokeModelWithResponseStream|2xx->5xx|$.__wire_eventstream__
Issue-Key: bedrock-runtime|InvokeModelWithResponseStream|2xx->5xx|$headers.content-type
Issue-Key: bedrock-runtime|InvokeModelWithResponseStream|2xx->5xx|$headers.x-amzn-bedrock-content-type
Issue-Key: bedrock-runtime|InvokeModelWithResponseStream|2xx->5xx|status
Issue-Commit: ceabbcc456fe2a783c33070041a4243561c3aa32

# Bedrock Runtime inference and streaming paths refused 500 AmbiguousService by sigv4-name routing

> _Note: this issue was identified against the code base at commit `ceabbcc456fe2a783c33070041a4243561c3aa32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
SHARED FAULT: the emulator's request router resolves an incoming request to an emulated service by its SigV4 signing name. Smithy (com-amazonaws-bedrockruntime-amazonbedrockfrontendservice): sdkId 'Bedrock Runtime', endpointPrefix 'bedrock-runtime', sigv4Name 'bedrock'. Six emulated Bedrock-family services declare sigv4Name 'bedrock' (bedrock, bedrock-runtime, bedrock-agent, bedrock-agent-runtime, bedrock-data-automation, bedrock-data-automation-runtime), so the resolution is ambiguous and the router answers HTTP 500 with errortype 'AmbiguousService' and the body "6 emulated services all claim requests for 'bedrock' ...". No Bedrock Runtime operation is reachable at all. Secondary defect visible in every cause: the refusal is serialized with content-type 'application/x-amz-json-1.1' even though this service's protocol is restJson1, whose responses (including errors) are 'application/json'.

TEN CAUSES, all on the inference / streaming surface. Each is a distinct operation+status contract that the router makes unreachable; a fixer must see all ten answer correctly.

Cause 1 of 10 — Converse, model at end-of-life (404).
AWS: Smithy Converse -> POST /model/{modelId}/converse, errors include ResourceNotFoundException (errorKind client, httpError 404, member `message`). The recording shows 404, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock/', content-type application/json, message example 'This model version has reached the end of its life. Please refer to the AWS documentation for more details.' (message text is an example; the status and error code are the requirement).
Emulator: HTTP 500, x-amzn-errortype 'AmbiguousService', content-type application/x-amz-json-1.1, body is the router's ambiguity text.
Why: an SDK client gets a retryable InternalServerError instead of ResourceNotFoundException, so model-availability handling and retry policy both break.
Fix: the request router / service-resolution layer.
Traces: bedrock-runtime-converse-error-paths#task#1.

Cause 2 of 10 — Converse, conversation does not start with a user message (400).
AWS: ValidationException (Smithy: errorKind client, httpError 400, member `message`; it is a service-common error on every bedrock-runtime op). Recorded: 400, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/', content-type application/json; recorded message example 'A conversation must start with a user message. Try again with a conversation that starts with a user message.'
Emulator: 500 / AmbiguousService / application/x-amz-json-1.1.
Why: a malformed conversation is reported as a server fault, so clients retry a request that can never succeed.
Fix: the request router / service-resolution layer (then Converse request validation).
Traces: bedrock-runtime-converse-error-paths#task#2 (cluster also covers bedrock-runtime-converse-error-paths-and-recovery#task#1, bedrockruntime-converse-envelope-and-rejections#task#3 and #task#4).

Cause 3 of 10 — Converse success envelope (200).
AWS: Smithy Converse -> ConverseResponse; recorded 200 with content-type application/json and body members output.message{role,content[].text}, stopReason, usage{inputTokens,outputTokens,totalTokens,serverToolUsage}, metrics{latencyMs}. (Token counts, latency and generated text are environment-specific examples; the member set and status are the requirement.)
Emulator: 500 / AmbiguousService; output, stopReason, usage and metrics are all absent.
Why: the happy path of the primary inference API returns nothing a client can deserialize.
Fix: the request router / service-resolution layer (then the Converse handler and serializer).
Traces: bedrockruntime-converse-envelope-and-rejections#task#1.

Cause 4 of 10 — ConverseStream success, event-stream frames (200).
AWS: Smithy ConverseStream -> POST /model/{modelId}/converse-stream, output ConverseStreamResponse with member `stream` (@httpPayload, @streaming). Recorded: 200, content-type application/vnd.amazon.eventstream, frame order messageStart -> contentBlockDelta -> contentBlockStop -> messageStop -> metadata.
Emulator: 500 / AmbiguousService with content-type application/x-amz-json-1.1 and no event stream at all.
Why: streaming clients see no frames, so no incremental generation is possible and the eventstream content-type contract is violated.
Fix: the request router / service-resolution layer (then the ConverseStream handler and its event-stream serializer).
Traces: bedrock-runtime-converse-stream-basic#task#1 (cluster also covers bedrockruntime-streaming-frame-order#task#1).

Cause 5 of 10 — ConverseStream, invalid model identifier (400).
AWS: ValidationException, httpError 400, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/', content-type application/json; recorded message example 'The provided model identifier is invalid.'
Emulator: 500 / AmbiguousService / application/x-amz-json-1.1.
Why: a bad modelId is indistinguishable from a transient server failure.
Fix: the request router / service-resolution layer (then ConverseStream modelId validation).
Traces: bedrockruntime-streaming-frame-order#task#3.

Cause 6 of 10 — InvokeModel, model at end-of-life (404).
AWS: Smithy InvokeModel -> POST /model/{modelId}/invoke, errors include ResourceNotFoundException (httpError 404). Recorded: 404, x-amzn-errortype 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock/', content-type application/json.
Emulator: 500 / AmbiguousService.
Why: same as cause 1, on the non-streaming invoke path.
Fix: the request router / service-resolution layer.
Traces: bedrock-runtime-invoke-model-error-paths#task#1.

Cause 7 of 10 — InvokeModel, malformed model-specific request body (400).
AWS: ValidationException, httpError 400, content-type application/json; recorded message example 'Malformed input request, please reformat your input and try again.'
Emulator: 500 / AmbiguousService.
Why: a client that sends a body the model cannot parse is told the service failed, not that its request was invalid.
Fix: the request router / service-resolution layer (then InvokeModel body validation).
Traces: bedrock-runtime-count-tokens-before-invoke#task#3 (cluster also covers bedrock-runtime-invoke-model-error-paths#task#2 and bedrock-runtime-invoke-model-invalid-content-type#task#1).

Cause 8 of 10 — InvokeModel success envelope (200).
AWS: recorded 200, content-type application/json, body carrying the model's own payload (output.message, stopReason, usage with cacheReadInputTokenCount / cacheWriteInputTokenCount present).
Emulator: 500 / AmbiguousService; output, stopReason and usage absent.
Why: the non-streaming invoke happy path yields no model output.
Fix: the request router / service-resolution layer (then the InvokeModel handler and serializer).
Traces: bedrockruntime-converse-envelope-and-rejections#task#2.

Cause 9 of 10 — InvokeModelWithResponseStream success, including the bedrock content-type header (200).
AWS: Smithy InvokeModelWithResponseStream -> output members `body` (@httpPayload @streaming, required) and `contentType` bound to the required header X-Amzn-Bedrock-Content-Type. Recorded: 200, content-type application/vnd.amazon.eventstream, x-amzn-bedrock-content-type 'application/json', one 'chunk' frame carrying `bytes`.
Emulator: 500 / AmbiguousService, content-type application/x-amz-json-1.1, x-amzn-bedrock-content-type entirely absent, no frames.
Why: clients cannot stream, and even on a fixed body they could not learn the payload MIME type the model used, because the modeled required response header is missing.
Fix: the request router / service-resolution layer (then the InvokeModelWithResponseStream handler, its header binding and its event-stream serializer).
Traces: bedrock-runtime-invoke-model-response-stream#task#1.

Cause 10 of 10 — InvokeModelWithResponseStream success, chunk frames for a second model family (200).
AWS: as cause 9: 200, content-type application/vnd.amazon.eventstream, 'chunk' frames with `bytes`.
Emulator: 500 / AmbiguousService, no event stream.
Why: response streaming is unavailable for every model, not just one.
Fix: the request router / service-resolution layer (then the InvokeModelWithResponseStream handler and event-stream serializer).
Traces: bedrock-runtime-invoke-model-with-response-stream-titan#task#1 (cluster also covers bedrockruntime-streaming-frame-order#task#2).

RELATED OPEN ISSUES (same routing fault, different services — none covers bedrock-runtime): 20261022T042609Z-bedrock-data-automation-runtime-sigv4-name-routing-makes-all-bedrock-data-automation.md, 20261022T062300Z-forecastquery-is-entirely-unreachable-router-refuses-every-request.md, 20261022T071206Z-kinesis-analytics-v2-is-entirely-unreachable-router-refuses.md. A fix to the shared resolver would close all of them, but this issue's replay keys must still pass.

## Evidence
Replay of trace set simparity-wire-aws-v1: 65 steps, 2 passed, 55 mismatched, score 3.1%. Emulator was up (/__admin/health 200, data listener on :41535 accepting), bootstrap_signal=false, state_reset_failures=0 — so these are live wrong answers, not a bootstrap gap.

Every one of the ten causes above produced the same actual response shape:
  actual status 500 (expected 404 / 400 / 200 depending on the cause)
  header x-amzn-errortype: 'AmbiguousService' (expected 'ResourceNotFoundException:http://internal.amazon.com/coral/com.amazon.bedrock/' or 'ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/', or no error header at all on a 200)
  header content-type: 'application/x-amz-json-1.1' (expected 'application/json', or 'application/vnd.amazon.eventstream' on the streaming causes)
  body $.message: "6 emulated services all claim requests for 'bedrock': bedrock, bedrock, bedrock, bedrock, bedrock, bedrock; the request is rejected rather than served by an arbitrary one"

Success-path causes additionally reported the whole modeled body as missing:
  Converse 200: $.output, $.stopReason, $.usage, $.metrics all missing
  InvokeModel 200: $.output, $.stopReason, $.usage all missing
  ConverseStream 200: $.__wire_eventstream__ missing (expected frames messageStart, contentBlockDelta, contentBlockStop, messageStop, metadata)
  InvokeModelWithResponseStream 200: $.__wire_eventstream__ missing (expected a 'chunk' frame with keys ['bytes','p']) and header x-amzn-bedrock-content-type missing (expected 'application/json')

Model evidence: smithy_query service_info on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json returns sigv4Name 'bedrock' with endpointPrefix 'bedrock-runtime' and protocols ['restJson1']; the control-plane model services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/... returns the SAME sigv4Name 'bedrock' with endpointPrefix 'bedrock'. The signing name is therefore not a unique key for service resolution, while the endpoint prefix is.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Representative step: fixture 'bedrock-runtime-converse-error-paths', step 'task#1' — POST /model/{modelId}/converse signed for sigv4 service name 'bedrock' against a model version that is end-of-life. Expected HTTP 404 with content-type application/json and x-amzn-errortype 'ResourceNotFoundException:...'; observed HTTP 500 with content-type application/x-amz-json-1.1 and x-amzn-errortype 'AmbiguousService'.
4. Repeat for the other nine causes, listed in trace_ids as fixture#step: converse validation (bedrock-runtime-converse-error-paths#task#2), converse success (bedrockruntime-converse-envelope-and-rejections#task#1), converse-stream success (bedrock-runtime-converse-stream-basic#task#1), converse-stream bad modelId (bedrockruntime-streaming-frame-order#task#3), invoke-model 404 (bedrock-runtime-invoke-model-error-paths#task#1), invoke-model malformed body (bedrock-runtime-count-tokens-before-invoke#task#3), invoke-model success (bedrockruntime-converse-envelope-and-rejections#task#2), response-stream success + x-amzn-bedrock-content-type (bedrock-runtime-invoke-model-response-stream#task#1), response-stream chunk frames (bedrock-runtime-invoke-model-with-response-stream-titan#task#1).
5. Every one of them answers 500 AmbiguousService. Fixing the resolver (dispatch on endpoint prefix / host and URI path instead of the SigV4 signing name) is necessary before any of the ten can be checked; re-replay all ten keys afterwards.
