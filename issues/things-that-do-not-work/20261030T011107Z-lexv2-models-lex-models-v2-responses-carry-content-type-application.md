Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-bot-locale-export-lifecycle
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-bot-locale-export-lifecycle lex-models-v2-bot-locale-slot-pipeline lex-models-v2-bot-resource-policy-lifecycle lex-models-v2-bot-version-alias-lifecycle
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|CreateBot|2xx->2xx|$headers.content-type
Issue-Key: lexv2-models|DescribeBot|2xx->2xx|$headers.content-type
Issue-Key: lexv2-models|CreateBotLocale|2xx->2xx|$.speechDetectionSensitivity
Issue-Key: lexv2-models|CreateBotLocale|2xx->2xx|$headers.content-type
Issue-Key: lexv2-models|DescribeBotLocale|2xx->2xx|$.intentsCount
Issue-Key: lexv2-models|DescribeBotLocale|2xx->2xx|$.slotTypesCount
Issue-Key: lexv2-models|DescribeBotLocale|2xx->2xx|$.speechDetectionSensitivity
Issue-Key: lexv2-models|DescribeBotLocale|2xx->2xx|$headers.content-type
Issue-Key: lexv2-models|DescribeBot|4xx->4xx|$.message
Issue-Key: lexv2-models|DescribeBot|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|CreateBotLocale|4xx->4xx|$.message
Issue-Key: lexv2-models|CreateBotLocale|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|CreateIntent|4xx->4xx|$.message
Issue-Key: lexv2-models|CreateIntent|4xx->4xx|$headers.content-type
Issue-Key: lexv2-models|DescribeBotLocale|2xx->2xx|$.botLocaleStatus
Issue-Key: lexv2-models|DescribeBotLocale|2xx->2xx|$.lastBuildSubmittedDateTime
Issue-Commit: f436928141e9968ccb87a436b810c90c79edbabf

# lex-models-v2 responses carry Content-Type application/json instead of application/x-amz-json-1.1

> _Note: this issue was identified against the code base at commit `f436928141e9968ccb87a436b810c90c79edbabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBot

## Description
Cause 1 of 1: every lexv2-models response the corpus replays — 2xx and 4xx alike — is serialized with the wrong Content-Type response header. This is a single defect in the service's response writer, not a per-operation one: it reproduces on CreateBot, DescribeBot, CreateBotLocale, DescribeBotLocale and CreateIntent, on 200, 202, 400 and 404 responses.

AWS: Smithy: LexModelBuildingServiceV2 (sdkId 'Lex Models V2', apiVersion 2020-08-07) is bound to aws.protocols#restJson1, and its operations are HTTP-bound (e.g. CreateBot PUT /bots code 202, DescribeBotLocale GET /bots/{botId}/botversions/{botVersion}/botlocales/{localeId} code 200). The concrete media type the Lex V2 model-building frontend puts on the wire is recorded behavior only: every recorded response in this corpus, success and error, carries 'Content-Type: application/x-amz-json-1.1'. That is the fixed, request-independent header value for this service — it has no variable part.

Emulator: the same responses are written with 'Content-Type: application/json'. Status codes and (for the steps covered by this issue) bodies match; only the media type diverges.

Why: a client that dispatches on the response media type — a hand-rolled client, a proxy, a recorder/replayer or a strict content-negotiation layer in front of the emulator — sees a different document type than the service announces, so responses that must be treated as Amazon JSON 1.1 are routed to a plain-JSON path or rejected. Fixtures recorded against AWS and replayed against the emulator can never match on headers, which masks or fabricates divergences for every operation of the service.

Fix: the lexv2-models response serializer / HTTP response writer shared by the service's handlers — the place the Content-Type header is set for both success and error responses. It must emit 'application/x-amz-json-1.1' for every modeled response of this service rather than the protocol-default 'application/json'.

Traces: lex-models-v2-bot-locale-export-lifecycle#task#2 (CreateBot, 202), #task#3 (DescribeBot, 200), #task#4 (CreateBotLocale, 202), #task#5 (DescribeBotLocale, 200); lex-models-v2-bot-locale-slot-pipeline#task#11 (DescribeBot, 404), #task#12 (CreateBotLocale, 400), #task#13 (CreateIntent, 400); lex-models-v2-bot-version-alias-lifecycle#task#12 (DescribeBotLocale, 200). The clusters also cover lex-models-v2-bot-locale-slot-pipeline#task#2/#3/#4/#5, lex-models-v2-bot-resource-policy-lifecycle#task#2/#3 and lex-models-v2-bot-version-alias-lifecycle#task#3/#4/#5/#6.

## Evidence
Replay divergence, all eight clusters of this service: "$headers.content-type: value_mismatch (expected 'application/x-amz-json-1.1', got 'application/json')". Examples: CreateBot 202 -> 202 with header mismatch (lex-models-v2-bot-locale-export-lifecycle#task#2, cluster also covers lex-models-v2-bot-locale-slot-pipeline#task#2, lex-models-v2-bot-resource-policy-lifecycle#task#2, lex-models-v2-bot-version-alias-lifecycle#task#3); DescribeBot 404 -> 404 with the same header mismatch (lex-models-v2-bot-locale-slot-pipeline#task#11); CreateIntent 400 -> 400 with the same header mismatch (lex-models-v2-bot-locale-slot-pipeline#task#13). Expected: Content-Type: application/x-amz-json-1.1. Actual: Content-Type: application/json.

## Steps to Reproduce
1. Boot the emulator from the checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture lex-models-v2-bot-locale-export-lifecycle step task#2 (CreateBot) and inspect the response headers: the status is 202 as recorded but Content-Type is application/json.
3. Repeat for any other lexv2-models step — e.g. lex-models-v2-bot-locale-slot-pipeline#task#11 (DescribeBot 404) or #task#13 (CreateIntent 400) — and the same header divergence appears on error responses too.
