Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-model-invocation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-model-invocation
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock-runtime|InvokeModel|2xx->4xx|$.output
Issue-Key: bedrock-runtime|InvokeModel|2xx->4xx|$.stopReason
Issue-Key: bedrock-runtime|InvokeModel|2xx->4xx|$.usage
Issue-Key: bedrock-runtime|InvokeModel|2xx->4xx|status
Issue-Key: bedrock-runtime|InvokeModel|4xx->4xx|$.message
Issue-Key: bedrock-runtime|InvokeModel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|InvokeModel|4xx->4xx|status
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# Bedrock Runtime model invocation answers 404 ResourceNotFoundException instead of inference result and ValidationException

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModel

## Description
Two causes on the bedrock-runtime inference path; both come from the same gap - the emulator has no foundation model inventory, so every invocation resolves to 'not found'. Note the open issue 'Bedrock Runtime inference and streaming paths refused 500 AmbiguousService by sigv4-name routing' describes a 500 routing refusal; the path now routes and answers 404, so these are different causes that issue does not carry.

Cause 1 of 2: invoking a valid model returns 404 instead of the inference response.
AWS: Smithy bedrock-runtime models the invocation path with http 200 on success; the recorded 200 body is the Converse-shaped envelope with the modeled @required members output (ConverseOutput), stopReason (StopReason enum) and usage (TokenUsage: inputTokens/outputTokens/totalTokens plus cache counts). ResourceNotFoundException (httpError 404) is modeled only for a model that does not exist.
Emulator: 404, no body members at all.
Why: no client can run inference against the emulator - the single most common bedrock-runtime call fails, and a caller that retries on 404 as a propagation delay never succeeds.
Fix: the invocation handler plus the model inventory/state it resolves modelId against; it must resolve the known foundation model ids and return a 200 envelope carrying output, stopReason and usage. The recorded assistant text and token counts are generated values and are examples only - the requirement is the status and the presence/shape of those members.
Traces: bedrock-model-invocation#task#1.

Cause 2 of 2: an unknown model id gets the wrong status and the wrong error code.
AWS: 400 with x-amzn-errortype ValidationException (Smithy: ValidationException, errorKind client, httpError 400), message 'The provided model identifier is invalid.' - a fixed service message for a modelId the service cannot interpret.
Emulator: 404 with x-amzn-errortype ResourceNotFoundException and message 'The provided model identifier is invalid: nonexistent.model-v1'.
Why: SDK retry and error classification differ between the two - ValidationException/400 is a non-retryable client fault a caller surfaces immediately, while ResourceNotFoundException/404 is routinely treated as eventual-consistency and retried; a caller probing model availability also branches on the error code and takes the wrong branch.
Fix: modelId resolution in the invocation handler - an uninterpretable identifier must raise ValidationException (400), reserving ResourceNotFoundException for a well-formed identifier naming an absent resource.
Traces: bedrock-model-invocation#task#2 (cluster also covers bedrock-model-invocation#task#3).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 (bedrock-model-invocation#task#1, InvokeModel): status expected 200, got 404; $.output expected {'message': {'content': [{'text': '...'}], 'role': 'assistant'}}, got None; $.stopReason expected 'end_turn', got None; $.usage expected {'inputTokens': 2, 'outputTokens': 24, 'totalTokens': 26, 'cacheReadInputTokenCount': 0, 'cacheWriteInputTokenCount': 0}, got None.

Cause 2 (bedrock-model-invocation#task#2, cluster also affecting #task#3, InvokeModel with modelId 'nonexistent.model-v1'): status expected 400, got 404; $headers.x-amzn-errortype expected 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.bedrock/' (i.e. error code ValidationException), got 'ResourceNotFoundException'; $.message expected 'The provided model identifier is invalid.', got 'The provided model identifier is invalid: nonexistent.model-v1'.

Model evidence (smithy_query, services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json): InvokeModel http {method POST, code 200} with errors including ValidationException (client, 400) and ResourceNotFoundException (client, 404); Converse output models output/stopReason/usage/metrics as @required and is the envelope the recording carries. Generated text, token counts and the specific model id are environment examples, not requirements.

## Steps to Reproduce
1. Boot the emulator built from mainline.
2. POST to the bedrock-runtime invocation path for a known foundation model (as recorded in fixture bedrock-model-invocation step task#1) with a single user message. Expect 200 and a body carrying output.message.content, stopReason and usage.{inputTokens,outputTokens,totalTokens}; the emulator answers 404 with an empty envelope.
3. Repeat with modelId 'nonexistent.model-v1'. Expect 400 with header x-amzn-errortype naming ValidationException; the emulator answers 404 with ResourceNotFoundException.
4. Or replay the corpus and inspect fixture bedrock-model-invocation steps task#1, task#2 and task#3.
