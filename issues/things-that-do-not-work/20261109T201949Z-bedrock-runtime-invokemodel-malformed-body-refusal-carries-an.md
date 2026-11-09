Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-runtime-count-tokens-before-invoke
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-runtime-count-tokens-before-invoke bedrock-runtime-invoke-model-error-paths
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|InvokeModel|4xx->4xx|$.message
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# bedrock-runtime InvokeModel malformed-body refusal carries an emulator-authored message instead of AWS's fixed malformed-input sentence

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModel

## Description
One cause, observed on the wire corpus in two independent fixtures that reach the same InvokeModel body-parse site.

Cause 1 of 1: InvokeModel called with a request body that is not parseable JSON is correctly refused with HTTP 400 / ValidationException, but the `message` member carries a sentence the emulator wrote rather than the sentence the service emits.

AWS: Smithy com.amazonaws.bedrockruntime#InvokeModel (POST /model/{modelId}/invoke) models ValidationException with errorKind=client, @httpError 400; `body` is the @httpPayload blob (@length max 25000000, @sensitive) and `modelId` the required @httpLabel. When the payload cannot be parsed as JSON for the target model, the service refuses at the API boundary — the body is never forwarded to the model — with HTTP 400, error code ValidationException and message 'Malformed input request, please reformat your input and try again.'. The status and code are modeled; the exact sentence is recorded behavior only. That text is entirely FIXED: it names no member, no value, no byte offset, no model id and no request id, so there is NO variable part to be filled from the request or from emulator state. The whole string is the requirement.

Emulator: status 400 and code ValidationException are both correct, but message = 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' — a different, locally-authored sentence. The substituted text is byte-identical across both diverging fixtures, which shows one shared parse-failure site rather than two independent wordings.

Why: the error message is a requirement, not decoration. A client, SDK retry/recovery policy, negative test or support tool distinguishes 'the body could not be parsed' from every other ValidationException raised by the same operation (unknown modelId, out-of-range inferenceConfig, oversized payload, bad guardrail header) by matching AWS's sentence. With the sentence replaced, all of those refusals look alike: message assertions and golden files that pass against the emulator fail against AWS, and log-based alarms keyed on the malformed-input sentence never fire.

Fix: the InvokeModel handler's body-parse / input-validation stage in the bedrock-runtime service crate — the site that turns a JSON deserialization failure of the @httpPayload `body` into a ValidationException. It must emit the service's fixed malformed-input sentence for the parse failure instead of describing the syntax error in its own words. The status and code paths already behave; only the message construction diverges.

NOTE on neighbouring open issues (neither carries this wire fixture's replay key, so neither covers this step): 20261109T043951Z-bedrock-runtime-refusal-messages-are-re-worded-by.md files the same substituted sentence from the BEHAVIORAL corpus (checks invoke-model-malformed-json-body-rejected / malformed-request-body-not-forwarded-to-model) and is almost certainly the same underlying code site — fixing that one should close this, but this wire step must be re-replayed to confirm. 20261029T213834Z-bedrock-runtime-error-paths-diverge-wrong-status-and.md files the same CLASS of defect (re-worded validation messages) for ApplyGuardrail's input-format refusal, InvokeGuardrailChecks' enum-set ordering and two async-invoke status/code defects — InvokeModel's parse-failure message is not among them.

Traces: bedrock-runtime-count-tokens-before-invoke#task#3 (representative) and bedrock-runtime-invoke-model-error-paths#task#2 — two fixtures, both InvokeModel, both observing the same substituted sentence.

## Evidence
Wire replay of trace set simparity-wire-aws-v1, divergence cluster service=bedrock-runtime|InvokeModel|status=400->400|keys=$.message, verdict=mismatch, affected_count=2.

Expected status 400, actual status 400 (correct). Expected error code ValidationException, actual ValidationException (correct).

$.message: value_mismatch
  expected: 'Malformed input request, please reformat your input and try again.'
  actual:   'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.'

Message anatomy: the expected string is 100% fixed text — there is no slot in it for a member name, value, limit, id or timestamp, so nothing in it is drawn from the request or from emulator state and nothing in it is per-run. The comparison is therefore an exact-string requirement, and the divergence is not a per-run value difference (no request id, clock time or generated id is involved).

Failing steps:
  - fixture bedrock-runtime-count-tokens-before-invoke, step task#3 (InvokeModel issued after a CountTokens call)
  - fixture bedrock-runtime-invoke-model-error-paths, step task#2
Both returned the identical substituted sentence.

Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json): InvokeModel http {method: POST, uri: /model/{modelId}/invoke, code: 200}; errors include ValidationException (errorKind=client, httpError=400); input member `body` target Body, httpPayload=true, @length max 25000000, @sensitive.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout.
2. Call replay_wire_corpus() and look at the cluster keyed service=bedrock-runtime|InvokeModel|status=400->400|keys=$.message.
3. Specifically, replay fixture bedrock-runtime-count-tokens-before-invoke step task#3, and fixture bedrock-runtime-invoke-model-error-paths step task#2.
4. Equivalently by hand: POST /model/<modelId>/invoke with Content-Type: application/json and a body that is not valid JSON (e.g. 'this is not json at all {{{').
5. Observe HTTP 400 with x-amzn-errortype ValidationException and body $.message = 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' where AWS returns $.message = 'Malformed input request, please reformat your input and try again.'
6. After fixing, re-replay BOTH listed fixture#step bindings — the substituted sentence is emitted from one shared site, so a fix must clear both.
