Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-bot-locale-slot-pipeline
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-bot-locale-slot-pipeline lex-models-v2-bot-version-alias-lifecycle
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|CreateSlotType|2xx->2xx|$headers.content-type
Issue-Commit: 2b4fec6236c4ef6c466d7be68e09ae75437e2d44

# CreateSlotType response Content-Type is application/json not application/x-amz-json-1.1

> _Note: this issue was identified against the code base at commit `2b4fec6236c4ef6c466d7be68e09ae75437e2d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlotType

## Description
Cause 1 of 1: the CreateSlotType success response is serialized with the protocol-default media type instead of the media type this service announces, so the response header diverges while the status and the body match.

AWS: Smithy (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2): CreateSlotType is bound to LexModelBuildingServiceV2 (aws.protocols#restJson1) as PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/slottypes with success code 200, output CreateSlotTypeResponse (slotTypeId, slotTypeName, description, slotTypeValues, valueSelectionSetting, parentSlotTypeSignature, botId, botVersion, localeId, creationDateTime, externalSourceSetting, compositeSlotTypeSetting). The concrete media type is recorded behavior only: the recorded CreateSlotType 200 response carries 'Content-Type: application/x-amz-json-1.1', which is the fixed, request-independent header value for this service and has no variable part.

Emulator: the same CreateSlotType 200 response is written with 'Content-Type: application/json'. The status (200) and every body member match the recording; only the media type diverges.

Why: a client that dispatches on the response media type — a hand-rolled client, a proxy, a recorder/replayer, or a strict content-negotiation layer in front of the emulator — sees a different document type than the service announces, so a response that must be handled as Amazon JSON 1.1 is routed to a plain-JSON path or rejected outright. Header-asserting fixtures recorded against AWS can never match on this operation, which masks real divergences and fabricates false ones.

Fix: the lexv2-models response serializer / HTTP response writer used by the CreateSlotType handler — the place that sets the Content-Type header on success responses. It must emit 'application/x-amz-json-1.1' for this service's modeled responses rather than the restJson1 default 'application/json'.

Note: open issue 20261030T011107Z-lexv2-models-lex-models-v2-responses-carry-content-type-application.md describes the same underlying response-writer defect for CreateBot, DescribeBot, CreateBotLocale, DescribeBotLocale and CreateIntent, but it carries no replay key for CreateSlotType, so this operation's step is not exercised by that issue's pointer and is filed here.

Traces: lex-models-v2-bot-locale-slot-pipeline#task#6 (CreateSlotType, 200). The cluster also covers lex-models-v2-bot-version-alias-lifecycle#task#7.

## Evidence
Replay divergence (verdict=mismatch), divergence_key service=lexv2-models|CreateSlotType|status=200->200|keys=$headers.content-type: "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')". Expected status 200, actual status 200 — body members all match. Expected header: Content-Type: application/x-amz-json-1.1 (fixed text, no per-request part). Actual header: Content-Type: application/json. Affected steps in the cluster: lex-models-v2-bot-locale-slot-pipeline#task#6, lex-models-v2-bot-version-alias-lifecycle#task#7.

## Steps to Reproduce
1. Boot the emulator from the checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture lex-models-v2-bot-locale-slot-pipeline up to step task#6 (CreateSlotType, PUT /bots/{botId}/botversions/DRAFT/botlocales/{localeId}/slottypes) and inspect the response headers: the status is 200 and the body matches the recording, but Content-Type is application/json instead of application/x-amz-json-1.1.
3. The same header divergence reproduces on lex-models-v2-bot-version-alias-lifecycle#task#7 (CreateSlotType, 200).
