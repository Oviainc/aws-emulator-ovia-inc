Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-catalog-list-foundation-models
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-catalog-list-foundation-models
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].batchSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].converse
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].latencyOptimizationSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].modelFamily
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].batchSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].converse
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].latencyOptimizationSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].modelFamily
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].batchSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.additionalRequestFieldsSchema
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.maxTokensMaximum
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.reasoningSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.stopSequencesDefault
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.systemRoleSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.userDocumentTypesSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.userImageTypesSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.userVideoTypesSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].explicitPromptCaching
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].featuresSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].inferenceAPIsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].intelligentPromptRouting
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"].modelLifecycle.startOfLifeTime
Issue-Related: 20261029T211357Z-bedrock-built-in-catalogue-is-wrong-foundation-models.md
Issue-Commit: 928c2eb9d028400e3e243ee9e4c9886012c2d3bc

# ListFoundationModels serves an incomplete foundation-model catalogue with wrong capability metadata

> _Note: this issue was identified against the code base at commit `928c2eb9d028400e3e243ee9e4c9886012c2d3bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFoundationModels

## Description
ONE cause: the emulator's seeded read-only foundation-model catalogue is not the catalogue AWS serves, so the GET /foundation-models discovery response is both short of most catalogued models and wrong about the capabilities of several models it does carry.

Cause 1 of 1: ListFoundationModels returns a 200 whose modelSummaries list is missing the large majority of catalogued modelIds, and whose present entries carry wrong capability/lifecycle metadata.

AWS: 200 with one modelSummaries entry per model in the account's region catalogue. Smithy: ListFoundationModels is @readonly, HTTP GET /foundation-models, code 200, output shape ListFoundationModelsResponse with member modelSummaries -> FoundationModelSummaryList; errors AccessDeniedException(403), ValidationException(400), ThrottlingException(429), InternalServerException(500). Smithy: FoundationModelSummary requires modelArn (pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}::foundation-model/...$) and modelId (BedrockModelId, @length max 140), and models modelName, providerName, inputModalities, outputModalities, responseStreamingSupported, customizationsSupported, inferenceTypesSupported and modelLifecycle. The further summary members the recording asserts — modelFamily, inferenceAPIsSupported, featuresSupported, guardrailsSupported, converse (additionalRequestFieldsSchema / maxTokensMaximum / reasoningSupported / stopSequencesDefault / systemRoleSupported / userDocumentTypesSupported / userImageTypesSupported / userVideoTypesSupported), batchSupported, explicitPromptCaching, intelligentPromptRouting, latencyOptimizationSupported, description, consoleIDEMetadata — are not in this repo's model copy, so for those: recorded behavior only; the recording shows AWS returning each of them on every summary, with nulls serialized for the unset optional ones (e.g. consoleIDEMetadata: null, description: null, maxTokensDefault: null) and modelLifecycle.status 'ACTIVE' with startOfLifeTime set.

Emulator: 200, but roughly 130 catalogued modelIds are absent from modelSummaries entirely — e.g. openai.gpt-6-astra, openai.gpt-6-luna, openai.gpt-6-sol, openai.gpt-6.1-sol, openai.gpt-5.4/5.5/5.6-*, anthropic.claude-opus-4-6-v1, anthropic.claude-opus-4-7/4-8, anthropic.claude-opus-5/5-5, anthropic.claude-sonnet-4-6/5/5-5, anthropic.claude-fable-5/5-1, nvidia.nemotron-nano-12b-v2, nvidia.nemotron-nano-3-30b, nvidia.nemotron-nano-9b-v2, nvidia.nemotron-super-3-120b, qwen.qwen3-*, mistral.ministral-3-*, mistral.voxtral-*, mistral.devstral-2-123b, mistral.magistral-small-2509, mistral.mistral-large-3-675b-instruct, minimax.minimax-m2/m2.1/m2.5, moonshot.kimi-k2-thinking, moonshotai.kimi-k3, google.gemma-3-4b/12b/27b-it, deepseek.v3.2, zai.glm-4.7/4.7-flash/5/5.3, xai.grok-4.6/4.7, writer.palmyra-vision-7b, and the whole stability.stable-image-* / upscale family. Summaries that ARE present diverge member-by-member: for amazon.titan-embed-image-v1 the emulator's batchSupported, converse, customizationsSupported[0], explicitPromptCaching, featuresSupported, inferenceAPIsSupported, inferenceTypesSupported[1], intelligentPromptRouting, latencyOptimizationSupported, modelFamily, modelName, modelLifecycle.startOfLifeTime and responseStreamingSupported all differ from the recording; cohere.embed-english-v3 diverges on batchSupported, converse, explicitPromptCaching, featuresSupported, inferenceAPIsSupported, inferenceTypesSupported[1], intelligentPromptRouting, latencyOptimizationSupported, modelFamily and modelLifecycle.startOfLifeTime; moonshotai.kimi-k2.5 diverges on batchSupported, every converse sub-member, explicitPromptCaching, featuresSupported, inferenceAPIsSupported, intelligentPromptRouting and modelLifecycle.startOfLifeTime. Entries such as amazon.titan-embed-text-v1, cohere.embed-multilingual-v3, amazon.titan-image-generator-v1, amazon.titan-text-lite-v1, stability.stable-diffusion-xl-v1 and amazon.titan-embed-g1-text-02 diverge as whole objects.

Why: ListFoundationModels is the discovery entry point for the whole service. A client that enumerates models and then selects one either cannot find the model it wants at all (it is absent from the list, so SDK/console flows and any model-picker built on this call silently exclude it), or it reads the wrong capability flags for a model that is listed and then makes a wrong call — attempting Converse on a model whose converse metadata it mis-read, assuming streaming where responseStreamingSupported is wrong, assuming fine-tuning where customizationsSupported is wrong, or assuming batch/prompt-caching/prompt-routing support that the real model does not have. Lifecycle decisions driven by modelLifecycle.startOfLifeTime are likewise taken against the wrong date.

Fix: the seeded foundation-model catalogue data for the bedrock service (the static catalogue the control-plane state is initialised from) and the ListFoundationModels handler's FoundationModelSummary projection, which must emit every modelled member plus the recorded capability/converse/lifecycle members for each catalogue entry rather than a reduced subset.

Related open issue that does NOT own this step (the replay reports no owner for it): 20261029T211357Z-bedrock-built-in-catalogue-is-wrong-foundation-models.md describes the same family of catalogue faults for GetPromptRouter/ListInferenceProfiles/ListPromptRouters, but it carries no replay key for the modelSummaries members that diverge here, so this ListFoundationModels divergence is unexercised by it.

Traces: bedrock-catalog-list-foundation-models#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Fixture bedrock-catalog-list-foundation-models, step task#1, operation ListFoundationModels, verdict mismatch, expected_status 200, actual_status 200, owned_by: [] (no existing issue carries this replay key), stub_path: "" (the operation is implemented).

divergence_key: service=bedrock|ListFoundationModels|status=200->200|keys=$.modelSummaries[modelId="..."] for ~130 model ids plus per-member keys on the entries that are present.

Expected vs actual, representative extracts from the replay detail:
- $.modelSummaries[modelId="openai.gpt-6-astra"]: missing — expected the full summary {modelArn 'arn:aws:bedrock:us-west-2::foundation-model/openai.gpt-6-astra', modelId 'openai.gpt-6-astra', modelName 'GPT-6 Astra', providerName 'OpenAI', modelFamily 'GPT-6', inputModalities ['TEXT','IMAGE'], outputModalities ['TEXT'], inferenceTypesSupported ['INFERENCE_PROFILE'], responseStreamingSupported true, guardrailsSupported true, converse {...}, featuresSupported {...}, batchSupported {...}, explicitPromptCaching {isSupported false}, intelligentPromptRouting {isSupported false}, latencyOptimizationSupported false, modelLifecycle {status 'ACTIVE', startOfLifeTime '2026-09-08T17:00:00Z'}}, got None.
- $.modelSummaries[modelId="nvidia.nemotron-nano-12b-v2"]: missing (expected full summary, got None); likewise for the qwen.*, stability.stable-image-*, zai.*, xai.*, minimax.*, google.gemma-3-*, mistral.ministral-3-*/voxtral-*, anthropic.claude-opus-4-6-v1/4-7/4-8/5/5-5, anthropic.claude-sonnet-4-6/5/5-5 and openai.gpt-5.x/gpt-6.x ids listed in divergence_key.
- Present-entry member divergences, e.g. $.modelSummaries[modelId="amazon.titan-embed-image-v1"].modelName, .modelFamily, .responseStreamingSupported, .customizationsSupported[0], .inferenceTypesSupported[1], .inferenceAPIsSupported, .featuresSupported, .batchSupported, .converse, .explicitPromptCaching, .intelligentPromptRouting, .latencyOptimizationSupported, .modelLifecycle.startOfLifeTime; $.modelSummaries[modelId="cohere.embed-english-v3"].* on the same members; $.modelSummaries[modelId="moonshotai.kimi-k2.5"].converse.additionalRequestFieldsSchema/.maxTokensMaximum/.reasoningSupported/.stopSequencesDefault/.systemRoleSupported/.userDocumentTypesSupported/.userImageTypesSupported/.userVideoTypesSupported.
- Whole-object divergences: $.modelSummaries[modelId="amazon.titan-embed-text-v1"], [modelId="cohere.embed-multilingual-v3"], [modelId="amazon.titan-image-generator-v1"], [modelId="amazon.titan-text-lite-v1"], [modelId="stability.stable-diffusion-xl-v1"], [modelId="amazon.titan-embed-g1-text-02"].

No status or error-code divergence is involved: both sides answer 200, so this is purely a response-body/catalogue-content defect. Generated ids, hosts and request ids are not asserted; the diverging values are catalogue content (model ids, names, families, capability flags and lifecycle dates), which is fixed data and not per-run.

Smithy evidence used: ListFoundationModels (GET /foundation-models, 200, output ListFoundationModelsResponse.modelSummaries -> FoundationModelSummaryList) and FoundationModelSummary (modelArn and modelId @required with the foundation-model ARN / BedrockModelId patterns; modelName, providerName, inputModalities, outputModalities, responseStreamingSupported, customizationsSupported, inferenceTypesSupported, modelLifecycle modelled). The additional members the recording asserts (modelFamily, converse, featuresSupported, inferenceAPIsSupported, batchSupported, explicitPromptCaching, intelligentPromptRouting, latencyOptimizationSupported, guardrailsSupported, description, consoleIDEMetadata) are recorded behavior only in this repo's model copy.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay fixture bedrock-catalog-list-foundation-models, step task#1 (GET /foundation-models, no query filters).
3. Compare the 200 response body against the recorded one: count modelSummaries and look up the recorded model ids (e.g. openai.gpt-6-astra, nvidia.nemotron-nano-12b-v2, qwen.qwen3-32b-v1:0, stability.stable-image-ultra-v1, zai.glm-5) — they are absent from the emulator's list.
4. For the ids that ARE present (amazon.titan-embed-image-v1, cohere.embed-english-v3, moonshotai.kimi-k2.5), diff each summary member-by-member: modelName, modelFamily, responseStreamingSupported, customizationsSupported, inferenceTypesSupported, inferenceAPIsSupported, featuresSupported, batchSupported, converse.*, explicitPromptCaching, intelligentPromptRouting, latencyOptimizationSupported and modelLifecycle.startOfLifeTime diverge from the recording.
5. Re-replay bedrock-catalog-list-foundation-models#task#1 after seeding the full catalogue and widening the summary projection; the step must pass with no modelSummaries key reported missing, extra or value-mismatched.
