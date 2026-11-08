Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-catalog-list-foundation-models
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-catalog-list-foundation-models
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-image-generator-v1:0"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="amazon.titan-text-lite-v1:0:4k"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.command-light-text-v14:7:4k"]
Issue-Key: bedrock|ListFoundationModels|2xx->2xx|$.modelSummaries[modelId="cohere.command-text-v14:7:4k"]
Issue-Related: 20261102T055258Z-bedrock-listfoundationmodels-model-summaries-diverge-converse-metadata-capability-flag.md
Issue-Related: 20261106T204258Z-bedrock-listfoundationmodels-foundation-model-catalog-seed-diverges-from.md
Issue-Commit: d259c56cebef1294bf0f05bb739042b6fdd3434e

# bedrock ListFoundationModels catalog diverges from AWS for newly recorded models and capability-flag shapes

> _Note: this issue was identified against the code base at commit `d259c56cebef1294bf0f05bb739042b6fdd3434e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFoundationModels

## Description
Single cause (Cause 1 of 1): the emulator's seeded foundation-model catalog served by ListFoundationModels does not match the catalog AWS returns, both in which models are present and in the per-model summary payload.

AWS: ListFoundationModels is modelled as GET /foundation-models returning 200 with `modelSummaries: FoundationModelSummaryList` (Smithy: ListFoundationModels, http {GET, /foundation-models}, code 200; errors AccessDeniedException 403 / InternalServerException 500 / ThrottlingException 429 / ValidationException 400). The on-disk Smithy FoundationModelSummary models `modelArn`, `modelId`, `modelName`, `providerName`, `inputModalities`, `outputModalities`, `responseStreamingSupported`, `customizationsSupported`, `inferenceTypesSupported`, `modelLifecycle`. The remaining members the recording carries — `converse` (with `maxTokensDefault`, `maxTokensMaximum`, `stopSequencesDefault`, `reasoningSupported.embedded`, `additionalRequestFieldsSchema`, `userImageTypesSupported`, `userDocumentTypesSupported`, `userVideoTypesSupported`), `featuresSupported` (`batchInference`, `provisionedThroughput`, `promptCaching`, `promptOptimization`, `countTokens`, `intelligentPromptRouting`), `batchSupported` (`baseModelSupported`, `customModelSupported`, `crossRegionSupported`), `explicitPromptCaching`, `intelligentPromptRouting`, `guardrailsSupported`, `inferenceAPIsSupported`, `latencyOptimizationSupported`, `modelFamily`, `description`, `modelLifecycle.startOfLifeTime` — are NOT in the checked-in model, so for those this is recorded behavior only: the recorded 200 response is the requirement. The recording shows AWS returning (a) catalog entries the emulator does not serve at all and omitting entries the emulator invents, (b) `featuresSupported.batchInference` as an OBJECT (`{"crossRegion": <bool>, "inRegion": <bool>}`) rather than a bare boolean, (c) `converse` metadata populated for every listed model, and (d) `converse.userImageTypesSupported` ordered `["jpeg", "png"]` with `userDocumentTypesSupported` absent for models that do not accept documents.

Emulator: the same step replays 200 but with a different catalog body. Representative diffs from the replay for `modelId="openai.gpt-6-astra"`: `converse.additionalRequestFieldsSchema` missing (expected the reasoning_config enum schema `{"reasoning_config":{"type":"enum","default":"medium","enum":["low","medium","high","xhigh","max"]}}`), `converse.maxTokensMaximum` missing (expected 131072), `converse.reasoningSupported.embedded` missing (expected false), `converse.userDocumentTypesSupported[0..8]` present as extra (`pdf, csv, doc, docx, xls, xlsx, html, txt, md`) where AWS returns none, `converse.userImageTypesSupported` ordered `png, jpeg` instead of `jpeg, png`, and `featuresSupported.batchInference` emitted as the boolean `false` where AWS returns the object `{"crossRegion": false, "inRegion": false}`. The same classes of diff repeat across the catalog: models AWS lists are absent or replaced (e.g. `amazon.nova-canvas-v1:0`, `amazon.titan-image-generator-v1`, `amazon.titan-image-generator-v1:0`, `amazon.titan-text-lite-v1`, `amazon.titan-text-lite-v1:0:4k`, `amazon.titan-text-premier-v1:0`, `anthropic.claude-3-5-sonnet-20241022-v2:0`, `anthropic.claude-3-haiku-20240307-v1:0`, `anthropic.claude-3-sonnet-20240229-v1:0`, `ai21.jamba-1-5-mini-v1:0`); `converse` wholly missing for sonic/lite/micro entries; `modelFamily`, `description`, `inferenceAPIsSupported`, `guardrailsSupported`, `latencyOptimizationSupported`, `explicitPromptCaching`, `intelligentPromptRouting`, `batchSupported.*`, `featuresSupported.*` and `modelLifecycle.startOfLifeTime` diverging per model; and newer recorded entries (`amazon.nova-2-5-sonic`, `anthropic.claude-fable-5`, `anthropic.claude-fable-5-1`, `anthropic.claude-opus-4-5-20251101-v1:0`, `anthropic.claude-opus-4-6-v1`, `anthropic.claude-opus-4-7`, `anthropic.claude-haiku-4-5-20251001-v1:0`, `openai.gpt-6-astra`) carrying incomplete capability metadata.

Why: ListFoundationModels is the discovery call every Bedrock client makes before it invokes a model. A client that filters on `inferenceTypesSupported`/`outputModalities`/`modelLifecycle` picks the wrong model or finds none; a client that reads `featuresSupported.batchInference` as the modelled object gets a JSON type error (boolean where a structure is expected) and fails to deserialise the whole list; a client that reads `converse.maxTokensMaximum` or `converse.additionalRequestFieldsSchema` to build a Converse request has no value and either omits required inference config or sends fields the model rejects. Because the wrong members are missing rather than wrong, this surfaces as a parse failure or an empty model list, not as a recognisable error.

Fix: the bedrock foundation-model catalog seed/state that backs ListFoundationModels (the catalog fixture the handler reads, and the serializer that shapes each FoundationModelSummary): bring catalog membership in line with the recording, emit `featuresSupported.batchInference` as a `{crossRegion, inRegion}` structure instead of a boolean, populate `converse` metadata for every catalogued model, omit `converse.userDocumentTypesSupported` for models that take no documents, and preserve AWS's member ordering for the `converse.user*TypesSupported` lists. Note: two open issues describe the same operation — 20261102T055258Z-bedrock-listfoundationmodels-model-summaries-diverge-converse-metadata-capability-flag.md and 20261106T204258Z-bedrock-listfoundationmodels-foundation-model-catalog-seed-diverges-from.md — but this replay reports the cluster as unowned (no open issue carries its replay key), and the diverging set now includes catalog entries those issues do not name, so it is filed separately and should be reconciled with them.

Traces: bedrock-catalog-list-foundation-models#task#1 (ListFoundationModels, expected 200 / actual 200 with a mismatched body).

## Evidence
Fixture bedrock-catalog-list-foundation-models, step task#1, operation ListFoundationModels, verdict mismatch, expected_status 200, actual_status 200.

Expected (AWS recording) vs actual (emulator), for modelId="openai.gpt-6-astra":
- $.modelSummaries[modelId="openai.gpt-6-astra"].converse.additionalRequestFieldsSchema: expected '{"reasoning_config":{"type":"enum","default":"medium","enum":["low","medium","high","xhigh","max"]}}', got None (missing)
- $...converse.maxTokensMaximum: expected 131072, got None (missing)
- $...converse.reasoningSupported.embedded: expected False, got None (missing)
- $...converse.userDocumentTypesSupported[0..8]: expected None, got 'pdf','csv','doc','docx','xls','xlsx','html','txt','md' (extra)
- $...converse.userImageTypesSupported[0]: expected 'jpeg', got 'png'; [1]: expected 'png', got 'jpeg'
- $...featuresSupported.batchInference: type_mismatch, expected {'crossRegion': False, 'inRegion': False}, got False

The cluster's divergence_key lists the full diverging path set, which spans catalog membership (whole $.modelSummaries[modelId=...] entries expected but absent, e.g. ai21.jamba-1-5-mini-v1:0, amazon.nova-canvas-v1:0, amazon.titan-image-generator-v1, amazon.titan-text-premier-v1:0, anthropic.claude-3-5-sonnet-20241022-v2:0, anthropic.claude-3-haiku-20240307-v1:0, anthropic.claude-3-sonnet-20240229-v1:0) and per-model members (converse.*, featuresSupported.*, batchSupported.*, explicitPromptCaching, intelligentPromptRouting, guardrailsSupported, inferenceAPIsSupported, inferenceTypesSupported[*], inputModalities[*], outputModalities[*], latencyOptimizationSupported, modelFamily, modelName, description, responseStreamingSupported, modelLifecycle.startOfLifeTime / legacyTime / endOfLifeTime / publicExtendedAccessTime / status).

Model evidence: Smithy ListFoundationModels -> GET /foundation-models, 200, output ListFoundationModelsResponse.modelSummaries: FoundationModelSummaryList. Smithy FoundationModelSummary models only modelArn (required), modelId (required), modelName, providerName, inputModalities, outputModalities, responseStreamingSupported, customizationsSupported, inferenceTypesSupported, modelLifecycle; converse/featuresSupported/batchSupported/modelFamily/description and the other diverging members are not in the checked-in model, so the recorded 200 body is the only authority for them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture bedrock-catalog-list-foundation-models, step task#1: GET /foundation-models against the bedrock endpoint (no query filters).
3. Compare the 200 response body's modelSummaries against the recorded response: the emulator returns 200 but a different catalog.
4. Observe for modelId "openai.gpt-6-astra" that converse.maxTokensMaximum, converse.reasoningSupported.embedded and converse.additionalRequestFieldsSchema are absent, converse.userDocumentTypesSupported is present where the recording has none, converse.userImageTypesSupported is ordered png,jpeg instead of jpeg,png, and featuresSupported.batchInference is the boolean false instead of the object {crossRegion, inRegion}.
5. Observe that model ids present in the recording (e.g. amazon.nova-canvas-v1:0, amazon.titan-image-generator-v1, anthropic.claude-3-haiku-20240307-v1:0, ai21.jamba-1-5-mini-v1:0) are missing from the emulator's list.
