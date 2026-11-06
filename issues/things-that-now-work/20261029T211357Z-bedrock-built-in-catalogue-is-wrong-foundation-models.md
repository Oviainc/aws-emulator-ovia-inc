Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-catalog-get-prompt-router
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-catalog-get-prompt-router bedrock-catalog-list-foundation-models bedrock-catalog-list-inference-profiles bedrock-catalog-list-prompt-routers
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.createdAt
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.description
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.fallbackModel
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.models
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.promptRouterArn
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.promptRouterName
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.routingCriteria
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.status
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.type
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|$.updatedAt
Issue-Key: bedrock|GetPromptRouter|2xx->4xx|status
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.createdAt
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.description
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.fallbackModel.modelArn
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.models[*]
Issue-Key: bedrock|GetPromptRouter|2xx->2xx|$.updatedAt
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.enableAccessToAllModelsByDefault
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="ai21.jamba-1-5-mini-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-2-5-sonic"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-2-lite-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-2-sonic-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-canvas-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-lite-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-micro-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.nova-pro-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.rerank-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-g1-text-02"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].customizationsSupported[*]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].explicitPromptCaching
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].featuresSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].guardrailsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].inferenceAPIsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].inferenceTypesSupported[*]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].intelligentPromptRouting
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].modelLifecycle.startOfLifeTime
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].modelName
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-image-v1"].responseStreamingSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-text-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-text-v1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-embed-text-v2
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-image-generator-v1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-text-lite-v1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-text-premier-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-3-5-sonnet-20241022-v2
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-3-haiku-20240307-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-3-sonnet-20240229-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-fable-5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-fable-5-1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-haiku-4-5-20251001-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-4-1-20250805-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-4-5-20251101-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-4-6-v1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-4-7"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-4-8"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-opus-5-5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-sonnet-4-20250514-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-sonnet-4-5-20250929-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-sonnet-4-6"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-sonnet-5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="anthropic.claude-sonnet-5-5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.command-light-text-v14"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.command-text-v14"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].explicitPromptCaching
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].featuresSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].guardrailsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].inferenceAPIsSupported
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].inferenceTypesSupported[*]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].intelligentPromptRouting
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-english-v3"].modelLifecycle.startOfLifeTime
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-multilingual-v3
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-multilingual-v3"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.embed-v4
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.rerank-v3-5
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="deepseek.r1-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="deepseek.v3-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="deepseek.v3.2"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="google.gemma-3-12b-it"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="google.gemma-3-27b-it"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="google.gemma-3-4b-it"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="luma.ray-v2
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama3-1-70b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama3-1-8b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama3-3-70b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama3-70b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama3-8b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama4-maverick-17b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="meta.llama4-scout-17b-instruct-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="minimax.minimax-m2"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="minimax.minimax-m2.1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="minimax.minimax-m2.5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.devstral-2-123b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.magistral-small-2509"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.ministral-3-14b-instruct"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.ministral-3-3b-instruct"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.ministral-3-8b-instruct"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.mistral-7b-instruct-v0
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.mistral-large-2402-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.mistral-large-2407-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.mistral-large-3-675b-instruct"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.mixtral-8x7b-instruct-v0
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.pixtral-large-2502-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.voxtral-mini-3b-2507"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="mistral.voxtral-small-24b-2507"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshot.kimi-k2-thinking"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k2.5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="moonshotai.kimi-k3"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="nvidia.nemotron-nano-12b-v2"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="nvidia.nemotron-nano-3-30b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="nvidia.nemotron-nano-9b-v2"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="nvidia.nemotron-super-3-120b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-5.4"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-5.5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-5.6-luna"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-5.6-sol"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-5.6-terra"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-6-astra"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-6-luna"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-6-sol"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-6.1-sol"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-oss-120b-1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-oss-20b-1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-oss-safeguard-120b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="openai.gpt-oss-safeguard-20b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="qwen.qwen3-235b-a22b-2507-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="qwen.qwen3-32b-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="qwen.qwen3-coder-30b-a3b-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="qwen.qwen3-coder-480b-a35b-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="qwen.qwen3-next-80b-a3b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="qwen.qwen3-vl-235b-a22b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.sd3-5-large-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-conservative-upscale-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-creative-upscale-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-diffusion-xl-v1"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-fast-upscale-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-control-sketch-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-control-structure-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-core-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-erase-object-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-inpaint-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-remove-background-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-search-recolor-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-search-replace-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-style-guide-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-image-ultra-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-outpaint-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="stability.stable-style-transfer-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="twelvelabs.pegasus-1-2-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="writer.palmyra-vision-7b"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="writer.palmyra-x4-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="writer.palmyra-x5-v1
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="xai.grok-4.6"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="xai.grok-4.7"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="zai.glm-4.7"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="zai.glm-4.7-flash"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="zai.glm-5"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="zai.glm-5.3"]
Issue-Key: bedrock|ListInferenceProfiles|2xx->2xx|$.inferenceProfileSummaries[*]
Issue-Key: bedrock|ListPromptRouters|2xx->2xx|$.promptRouterSummaries[*]
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# bedrock built-in catalogue is wrong: foundation models, inference profiles and prompt routers

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFoundationModels

## Description
FIVE causes, one shared fault: the emulator's seeded read-only catalogue (foundation models, system-defined inference profiles, default prompt routers) is not the catalogue AWS serves. Entries are missing, entries are ordered/identified differently, and the prompt routers point at foundation-model ARNs where AWS points at inference-profile ARNs. Every cause is a 200 whose body diverges (plus one 404 for a catalogue entry that is absent entirely).

Cause 1 of 5: GetPromptRouter for the default Nova prompt router returns 404 — that router is not in the catalogue at all.
AWS: 200 with promptRouterName, routingCriteria, description, createdAt, updatedAt, promptRouterArn, models, fallbackModel, status and type. Smithy: GetPromptRouter input promptRouterArn is a pattern-constrained @httpLabel ('...:default-prompt-router/[a-zA-Z0-9-:.]+'); output requires promptRouterName, routingCriteria, promptRouterArn, models, fallbackModel, status and type; status/type are enums (recorded values 'AVAILABLE' and 'default').
Emulator: 404 with none of those members — the default-prompt-router/amazon.nova entry does not exist.
Why: the documented default routers are a fixed part of every account; a client enumerating or addressing them hits a hard 404 and cannot exercise intelligent prompt routing at all.
Fix: the seeded prompt-router catalogue — it must contain every default router AWS ships, including the Nova family router.
Traces: bedrock-catalog-get-prompt-router#task#1.

Cause 2 of 5: GetPromptRouter for the default Claude prompt router returns the wrong body.
AWS: 200 whose models[] and fallbackModel.modelArn are INFERENCE-PROFILE ARNs ('arn:aws:bedrock:<region>:<account>:inference-profile/<profile-id>'), with description and createdAt/updatedAt present (createdAt/updatedAt are @timestampFormat date-time).
Emulator: 200 but models[] and fallbackModel.modelArn are FOUNDATION-MODEL ARNs ('arn:aws:bedrock:<region>::foundation-model/<model-id>'), naming different model versions than the router routes to, and description/createdAt/updatedAt are omitted entirely (description is modelled; createdAt/updatedAt are modelled timestamps).
Why: a caller that feeds the router's target ARNs onward gets a foundation-model ARN where an inference-profile ARN is required (different ARN grammar, different entitlement), and loses the router's description and lifecycle timestamps.
Fix: the seeded prompt-router catalogue's target/fallback ARNs and metadata, and GetPromptRouter's projection of description/createdAt/updatedAt.
Traces: bedrock-catalog-get-prompt-router#task#2 (also task#3 in the same cluster).

Cause 3 of 5: ListFoundationModels serves an incomplete and differently-shaped model catalogue.
AWS: 200 with enableAccessToAllModelsByDefault present and a modelSummaries entry per catalogued model, each carrying its full summary (modelArn, modelId, modelName, providerName, modelFamily, inputModalities/outputModalities, inferenceTypesSupported, inferenceAPIsSupported, featuresSupported, guardrailsSupported, responseStreamingSupported, customizationsSupported, modelLifecycle, converse metadata, batchSupported, explicitPromptCaching, intelligentPromptRouting, latencyOptimizationSupported).
Emulator: 200 but enableAccessToAllModelsByDefault is absent and ~130 catalogued modelIds are missing outright; several that are present (e.g. the titan-embed and cohere.embed entries) diverge in modelName, customizationsSupported, inferenceTypesSupported, inferenceAPIsSupported, featuresSupported, guardrailsSupported, explicitPromptCaching, intelligentPromptRouting, responseStreamingSupported and modelLifecycle.startOfLifeTime.
Why: ListFoundationModels is the discovery entry point; a client that picks a model from this list and then invokes it either cannot find the model it wanted or makes capability decisions (streaming, converse, guardrails, customization) from wrong flags.
Fix: the seeded foundation-model catalogue data and the ListFoundationModels summary projection (including the account-level enableAccessToAllModelsByDefault flag).
Traces: bedrock-catalog-list-foundation-models#task#1.

Cause 4 of 5: ListInferenceProfiles serves a different set of system-defined inference profiles.
AWS: 200 with inferenceProfileSummaries whose entries carry inferenceProfileName, inferenceProfileArn, inferenceProfileId, description, models[], status, type 'SYSTEM_DEFINED', createdAt and updatedAt.
Emulator: 200 but every index of inferenceProfileSummaries (93 positions diverged) holds a different profile than the recorded catalogue, and the entries served omit description, createdAt and updatedAt.
Why: a client resolving a profile id from this list gets a profile AWS does not have at that position, and loses the description/timestamps; cross-region routing decisions made from models[] are drawn from the wrong profile.
Fix: the seeded system-defined inference-profile catalogue (contents and ordering) and the ListInferenceProfiles summary projection.
Traces: bedrock-catalog-list-inference-profiles#task#1.

Cause 5 of 5: ListPromptRouters serves a different set of default prompt routers.
AWS: 200 with promptRouterSummaries covering each default router, each carrying createdAt, description, fallbackModel, models (inference-profile ARNs), promptRouterArn, promptRouterName, routingCriteria, status and type.
Emulator: 200 but the summaries are a different, shorter set (3 positions diverged): the Nova router from cause 1 is absent, the first entry is the Claude router, and its models/fallbackModel carry foundation-model ARNs with description/createdAt/updatedAt omitted, exactly as in cause 2.
Why: router enumeration cannot be trusted — the router a caller selects by index or name may not be the one AWS would return, and its targets are the wrong ARN kind.
Fix: the same seeded prompt-router catalogue as causes 1-2, plus the ListPromptRouters summary projection.
Traces: bedrock-catalog-list-prompt-routers#task#1.

Related open issues that do NOT cover these steps (no replay key for them): 20261029T172312Z-bedrock-reopens-bedrock-getfoundationmodel-the-catalogue-does-not-resolve.md and 20261023T232022Z-bedrock-catalogued-foundation-model-refused-as-invalid-responses.md.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 — bedrock-catalog-get-prompt-router#task#1 GetPromptRouter: expected 200, got 404; $.promptRouterArn (the default-prompt-router/amazon.nova:1 form), $.promptRouterName, $.description, $.models (two inference-profile ARNs), $.fallbackModel, $.routingCriteria, $.status 'AVAILABLE', $.type 'default', $.createdAt and $.updatedAt all missing.

Cause 2 — bedrock-catalog-get-prompt-router#task#2 GetPromptRouter (200/200, affected_count 2 with #task#3): $.fallbackModel.modelArn expected an 'inference-profile/us.anthropic.claude-...' ARN, got a 'foundation-model/anthropic.claude-...' ARN of a different version; $.models[0] and $.models[1] likewise inference-profile vs foundation-model ARNs; $.description, $.createdAt and $.updatedAt missing.

Cause 3 — bedrock-catalog-list-foundation-models#task#1 ListFoundationModels (200/200): $.enableAccessToAllModelsByDefault expected True, got None; ~130 $.modelSummaries[modelId="..."] entries missing (e.g. openai.gpt-6-astra, nvidia.nemotron-nano-12b-v2, anthropic.claude-opus-4-6-v1, qwen.*, stability.*); present entries diverge on modelName, customizationsSupported, inferenceTypesSupported[1], inferenceAPIsSupported, featuresSupported, guardrailsSupported, explicitPromptCaching, intelligentPromptRouting, responseStreamingSupported and modelLifecycle.startOfLifeTime.

Cause 4 — bedrock-catalog-list-inference-profiles#task#1 ListInferenceProfiles (200/200): $.inferenceProfileSummaries[0..92] all diverged; e.g. index 0 expected the 'US Anthropic Claude 3 Haiku' SYSTEM_DEFINED profile with description/createdAt/updatedAt, got a 'US Nova Lite' profile with no description, createdAt or updatedAt.

Cause 5 — bedrock-catalog-list-prompt-routers#task#1 ListPromptRouters (200/200): $.promptRouterSummaries[0], [1] and [2] diverged; index 0 expected the Nova default router (with description, createdAt, updatedAt and inference-profile ARNs), got the Anthropic router with foundation-model ARNs and no description/createdAt/updatedAt.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay bedrock-catalog-get-prompt-router#task#1, #task#2 and #task#3; bedrock-catalog-list-foundation-models#task#1; bedrock-catalog-list-inference-profiles#task#1; bedrock-catalog-list-prompt-routers#task#1.
3. GetPromptRouter on the default Nova router ARN returns 404; GetPromptRouter on the default Claude router returns foundation-model ARNs in models/fallbackModel and omits description/createdAt/updatedAt.
4. ListFoundationModels omits enableAccessToAllModelsByDefault and most catalogued modelIds; ListInferenceProfiles and ListPromptRouters return a different catalogue than the recording at every index.
