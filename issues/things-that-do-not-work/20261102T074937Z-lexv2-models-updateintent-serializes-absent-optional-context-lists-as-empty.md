Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-bot-version-alias-lifecycle
Issue-Step: task#10
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-bot-version-alias-lifecycle
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|UpdateIntent|2xx->2xx|$.inputContexts
Issue-Key: lexv2-models|UpdateIntent|2xx->2xx|$.outputContexts
Issue-Commit: 44569cf4143df1c541635e7034997227c77c67cb

# UpdateIntent serializes absent optional context lists as empty arrays instead of omitting them

> _Note: this issue was identified against the code base at commit `44569cf4143df1c541635e7034997227c77c67cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIntent

## Description
Cause 1 of 1: UpdateIntent's restJson1 response body materializes the optional list members `inputContexts` and `outputContexts` as empty JSON arrays when the request supplied no contexts, where AWS omits those members from the document entirely.

AWS: Smithy model services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json, operation UpdateIntent (PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}, success code 200). On UpdateIntentResponse, `inputContexts` targets InputContextsList (@length min 0 max 5) and `outputContexts` targets OutputContextsList (@length min 0 max 10); neither member is @required and neither carries a @default. Under restJson1 a structure member with no value is not serialized at all, so for an intent updated without contexts AWS returns a 200 body with no `inputContexts` and no `outputContexts` key — which is what the recording shows (expected value None for both). The rest of the response document and the 200 status match; this is purely presence/absence of the two optional list members. An empty list supplied EXPLICITLY by the request still round-trips as `[]`.

Emulator: answers 200 but emits `"inputContexts": []` and `"outputContexts": []`. Replay verdict `mismatch`, detail `$.inputContexts: type_mismatch (expected None, got [])` and `$.outputContexts: type_mismatch (expected None, got [])`; divergence key `service=lexv2-models|UpdateIntent|status=200->200|keys=$.inputContexts,$.outputContexts`. The serializer is defaulting an unset Option<Vec<_>> to an empty vector.

Why: a client cannot distinguish "no contexts configured" from "member not returned". In boto3 `'inputContexts' in response` is True against the emulator and False against AWS, so presence tests and `response.get('inputContexts')` guards take the wrong branch; in the typed SDKs the auto-construct-list marker is lost so `hasInputContexts()`/`hasOutputContexts()` report true, and a read-modify-write that feeds the UpdateIntent output into the next UpdateIntent now sends explicit empty lists where AWS sent nothing. Response-equality and recorded-response assertions over the intent document also fail.

Fix: the UpdateIntent response serialization / output-shape construction in the lexmodelsv2 crate (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/update_intent.rs). Absent optional list members must stay None through to serialization and be skipped in the JSON document rather than normalized to an empty collection.

Related: the open issue "CreateIntent serializes absent optional context lists as empty arrays instead of omitting them" (20261031T192848Z-lexv2-models-createintent-serializes-absent-optional-context-lists-as-empty.md) reports the same defect on CreateIntent. It carries the CreateIntent replay key only and does not own this UpdateIntent cluster (replay reports owned_by empty, stub_path empty for it), so this divergence is net-new; the two are very likely one shared normalization in the intent write path and should be fixed together.

Traces: lex-models-v2-bot-version-alias-lifecycle#task#10 (representative, only member of the cluster).

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `lex-models-v2-bot-version-alias-lifecycle`, step `task#10` (operation UpdateIntent, service lexv2-models):

  verdict: mismatch
  expected_status: 200   actual_status: 200
  detail:
    $.inputContexts: type_mismatch (expected None, got [])
    $.outputContexts: type_mismatch (expected None, got [])
  divergence_key: service=lexv2-models|UpdateIntent|status=200->200|keys=$.inputContexts,$.outputContexts
  affected (1): lex-models-v2-bot-version-alias-lifecycle#task#10
  stub_path: (empty)   owned_by: (empty)

Expected (AWS, recorded): 200 with a JSON document containing NO `inputContexts` key and NO `outputContexts` key.
Actual (emulator): 200 with `"inputContexts": []` and `"outputContexts": []` present.

Model basis: UpdateIntentResponse.inputContexts -> InputContextsList (@length min 0, max 5), UpdateIntentResponse.outputContexts -> OutputContextsList (@length min 0, max 10); neither @required, neither with @default; protocol restJson1, so an unset member is not serialized.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm the lexv2-models endpoint is serving.
2. CreateBot, CreateBotLocale (DRAFT + locale), CreateIntent without contexts.
3. Call UpdateIntent on that intent WITHOUT `inputContexts` or `outputContexts` — e.g. PUT /bots/{botId}/botversions/DRAFT/botlocales/{localeId}/intents/{intentId} with body {"intentName": "<name>", "description": "<new description>"}.
4. Inspect the raw 200 response body: the emulator includes `"inputContexts": []` and `"outputContexts": []`; AWS omits both keys. In boto3, `'inputContexts' in response` is True here and False against AWS.
5. Equivalently, replay fixture `lex-models-v2-bot-version-alias-lifecycle` step `task#10` from trace set simparity-wire-aws-v1 and observe the two `type_mismatch (expected None, got [])` entries.
