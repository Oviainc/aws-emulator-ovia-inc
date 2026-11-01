Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-catalog-list-foundation-models
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-catalog-list-foundation-models
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].batchSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].explicitPromptCaching
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].featuresSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].guardrailsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].inferenceAPIsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].intelligentPromptRouting
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].latencyOptimizationSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].modelFamily
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].modelLifecycle.startOfLifeTime
Issue-Related: 20261029T211357Z-bedrock-built-in-catalogue-is-wrong-foundation-models.md
Issue-Commit: c58e269113894f48dd3e032b4127d0ccc7e88163

# bedrock ListFoundationModels serves an incomplete foundation-model catalogue

> _Note: this issue was identified against the code base at commit `c58e269113894f48dd3e032b4127d0ccc7e88163`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFoundationModels

## Description
ONE cause: the emulator's seeded read-only foundation-model catalogue is not the catalogue AWS serves on GET /foundation-models, and the account-level access flag is omitted from the response body.

Cause 1 of 1: ListFoundationModels returns 200 with a catalogue that is missing most modelIds, omits enableAccessToAllModelsByDefault, and mis-populates the summaries it does serve.
AWS: 200 on GET /foundation-models (Smithy: ListFoundationModels, http GET /foundation-models, code 200; output shape ListFoundationModelsResponse.modelSummaries -> FoundationModelSummaryList). Each FoundationModelSummary carries the required members modelArn (pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/...$) and modelId (BedrockModelId, @length max 140), plus modelName, providerName, inputModalities, outputModalities, responseStreamingSupported, customizationsSupported, inferenceTypesSupported and modelLifecycle. The recording additionally shows members the on-disk model does not declare — recorded behavior only — a top-level enableAccessToAllModelsByDefault boolean (recorded true) and per-summary modelFamily, inferenceAPIsSupported, featuresSupported, guardrailsSupported, explicitPromptCaching, intelligentPromptRouting, latencyOptimizationSupported, batchSupported and converse metadata. Every model AWS catalogues in the region appears exactly once with that full summary.
Emulator: 200, but the top-level enableAccessToAllModelsByDefault is absent entirely, and roughly 130 catalogued modelIds are not in modelSummaries at all (across every provider family: openai.*, anthropic.*, amazon.nova*, amazon.titan-*, cohere.*, deepseek.*, google.gemma-*, meta.llama*, minimax.*, mistral.*, moonshot*, nvidia.nemotron-*, qwen.*, stability.*, twelvelabs.*, writer.*, xai.*, zai.*). Several summaries that ARE served diverge member-by-member from the recording — e.g. the titan-embed and cohere.embed entries differ on modelName, customizationsSupported, inferenceTypesSupported, inferenceAPIsSupported, featuresSupported, guardrailsSupported, explicitPromptCaching, intelligentPromptRouting, responseStreamingSupported and modelLifecycle.startOfLifeTime.
Why: ListFoundationModels is the discovery entry point for the whole service. A client that enumerates models and then addresses one by modelId or modelArn cannot find most of the models AWS offers, so every downstream call (GetFoundationModel, provisioned throughput, invocation/customization jobs, prompt routing) fails on an id the real service accepts. For the models that are listed, the capability flags are wrong, so a client decides incorrectly whether it may stream, use converse, apply guardrails, customise, batch or cache prompts; and with enableAccessToAllModelsByDefault missing, a client cannot tell whether model access needs to be requested at all.
Fix: the seeded foundation-model catalogue data for the bedrock control-plane service and the ListFoundationModels handler's summary projection — the catalogue must contain every model AWS catalogues with its full summary, and the handler must serialize the account-level enableAccessToAllModelsByDefault flag alongside modelSummaries. This is catalogue/state plus serializer, not a per-request validation issue. No status or error code diverges here: the status is 200 on both sides and no error message is involved.
Traces: bedrock-catalog-list-foundation-models#task#1.

Related open issue that does NOT cover this step: 20261029T211357Z-bedrock-built-in-catalogue-is-wrong-foundation-models.md describes the same family of catalogue faults and even names the fixture in its Issue-Trace-Ids, but it records it WITHOUT a step ("bedrock-catalog-list-foundation-models" with no #task#1), so it carries no replay key for this step and the replay reports the cluster as unowned. This issue supplies the missing fixture#step binding so the fix is gated on a replay that actually exercises ListFoundationModels.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, bootstrap_signal false).

bedrock-catalog-list-foundation-models#task#1 ListFoundationModels, verdict mismatch, expected status 200, actual status 200, owned_by [] (no existing issue carries this replay key), stub_path empty.

Diverging keys reported by the replay:
- $.enableAccessToAllModelsByDefault: missing (expected True, got None).
- ~130 $.modelSummaries[modelId="..."] entries missing outright, e.g. openai.gpt-6-astra (expected a full summary: modelArn arn:aws:bedrock:<region>::foundation-model/openai.gpt-6-astra, modelFamily GPT-6, modelName "GPT-6 Astra", providerName OpenAI, inputModalities [TEXT, IMAGE], outputModalities [TEXT], inferenceTypesSupported [INFERENCE_PROFILE], inferenceAPIsSupported with converse/invokeModel/openAiChatCompletions/openAiResponses, modelLifecycle.status ACTIVE, responseStreamingSupported true) — got None; likewise nvidia.nemotron-nano-12b-v2, anthropic.claude-opus-4-6-v1, anthropic.claude-sonnet-5, amazon.nova-2-lite-v1, cohere.embed-v4, deepseek.v3.2, meta.llama4-scout-17b-instruct-v1, mistral.mistral-large-3-675b-instruct, qwen.qwen3-vl-235b-a22b, stability.stable-image-ultra-v1, writer.palmyra-x5-v1, xai.grok-4.7, zai.glm-5.3 and the rest of that list.
- Summaries that are present but diverge, e.g. $.modelSummaries[modelId="amazon.titan-embed-image-v1"] on .modelName, .customizationsSupported[0], .inferenceTypesSupported[1], .inferenceAPIsSupported, .featuresSupported, .guardrailsSupported, .explicitPromptCaching, .intelligentPromptRouting, .responseStreamingSupported and .modelLifecycle.startOfLifeTime; $.modelSummaries[modelId="cohere.embed-english-v3"] on .explicitPromptCaching, .featuresSupported, .guardrailsSupported, .inferenceAPIsSupported, .inferenceTypesSupported[1], .intelligentPromptRouting and .modelLifecycle.startOfLifeTime; $.modelSummaries[modelId="moonshotai.kimi-k2.5"] on .batchSupported, .converse, .explicitPromptCaching, .featuresSupported, .guardrailsSupported, .inferenceAPIsSupported, .intelligentPromptRouting, .latencyOptimizationSupported, .modelFamily and .modelLifecycle.startOfLifeTime.

Model evidence: Smithy ListFoundationModels (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json) binds GET /foundation-models -> 200 with output member modelSummaries only; FoundationModelSummary requires modelArn and modelId and models modelName/providerName/inputModalities/outputModalities/responseStreamingSupported/customizationsSupported/inferenceTypesSupported/modelLifecycle. enableAccessToAllModelsByDefault and the extra capability members are recorded behavior only (not declared in the on-disk model), so the recording is the requirement for them.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire fixture step bedrock-catalog-list-foundation-models#task#1 (GET /foundation-models against the bedrock control-plane endpoint, no query filters).
3. Observe the 200 response body: the top-level enableAccessToAllModelsByDefault member is absent, and modelSummaries lacks ~130 of the modelIds the recording returns (search the body for openai.gpt-6-astra, anthropic.claude-opus-4-6-v1 or qwen.qwen3-vl-235b-a22b — none are present).
4. Compare the summaries that ARE present, e.g. amazon.titan-embed-image-v1 and cohere.embed-english-v3: modelName, customizationsSupported, inferenceTypesSupported, inferenceAPIsSupported, featuresSupported, guardrailsSupported, explicitPromptCaching, intelligentPromptRouting, responseStreamingSupported and modelLifecycle.startOfLifeTime differ from the recorded summary.
