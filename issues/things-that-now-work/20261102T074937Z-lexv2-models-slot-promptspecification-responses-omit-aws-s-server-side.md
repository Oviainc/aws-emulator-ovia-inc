Issue-Source: trace-sweep-divergence
Issue-Trace: lex-models-v2-bot-locale-slot-pipeline
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-models-v2-bot-locale-slot-pipeline lex-models-v2-bot-version-alias-lifecycle
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models|CreateSlot|2xx->2xx|$.valueElicitationSetting.promptSpecification.messageSelectionStrategy
Issue-Key: lexv2-models|CreateSlot|2xx->2xx|$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification
Issue-Key: lexv2-models|DescribeSlot|2xx->2xx|$.valueElicitationSetting.promptSpecification.messageSelectionStrategy
Issue-Key: lexv2-models|DescribeSlot|2xx->2xx|$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification
Issue-Commit: 44569cf4143df1c541635e7034997227c77c67cb

# Slot promptSpecification responses omit AWS's server-side defaults messageSelectionStrategy and promptAttemptsSpecification

> _Note: this issue was identified against the code base at commit `44569cf4143df1c541635e7034997227c77c67cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlot

## Description
The emulator echoes a slot's `valueElicitationSetting.promptSpecification` back exactly as the request supplied it, where AWS materializes two server-side defaults onto every stored prompt specification and returns them on every operation that reports the slot. Both causes below are the same defect in slot state/serialization, reached through two different operations (the write path and the read-back path), so a fix must make the stored prompt specification carry the defaults rather than patch one response.

Cause 1 of 2: CreateSlot (fixture lex-models-v2-bot-locale-slot-pipeline, step task#8).
AWS: recorded behavior only for the defaulting itself — Smithy model services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json, operation CreateSlot (PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}/slots, success 200). On PromptSpecification only `messageGroups` and `maxRetries` are @required; `messageSelectionStrategy` (enum MessageSelectionStrategy, values Random|Ordered) and `promptAttemptsSpecification` (map PromptAttemptsSpecificationMap, key enum PromptAttempt, value PromptAttemptSpecification) are optional and carry no @default in the model, so the model does not state the defaulting — the recording does. The recorded 200 response shows AWS filling both in when the request omitted them: `messageSelectionStrategy` = "Random", and `promptAttemptsSpecification` with one entry per prompt attempt (`Initial`, `Retry1`, `Retry2`), each entry being allowInterrupt=true, allowedInputTypes {allowAudioInput=true, allowDTMFInput=true}, audioAndDTMFInputSpecification {startTimeoutMs=4000, audioSpecification {endTimeoutMs=640, maxLengthMs=15000}, dtmfSpecification {deletionCharacter="*", endCharacter="#", endTimeoutMs=5000, maxLength=513}}, textInputSpecification {startTimeoutMs=30000}. These values are AWS's fixed defaults for an un-customized prompt, not values from the recorded request (the request carried only messageGroups and maxRetries); the number of attempt keys follows maxRetries+1 in the request, so a request with fewer retries yields fewer keys. Where the request DOES supply either member, AWS returns what the request supplied.
Emulator: answers 200 with the rest of the slot document correct, but with both members absent from `valueElicitationSetting.promptSpecification`. Replay verdict `mismatch`, detail `$.valueElicitationSetting.promptSpecification.messageSelectionStrategy: missing (expected 'Random', got None)` and `$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification: missing (expected {...Initial/Retry1/Retry2...}, got None)`; divergence key `service=lexv2-models|CreateSlot|status=200->200|keys=$.valueElicitationSetting.promptSpecification.messageSelectionStrategy,$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification`. It reproduces on both CreateSlot calls in the corpus (2 steps in the cluster), so it is the handler storing the request's prompt specification verbatim rather than anything fixture-specific.

Cause 2 of 2: DescribeSlot (fixture lex-models-v2-bot-locale-slot-pipeline, step task#9).
AWS: same model file, operation DescribeSlot (GET /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}/slots/{slotId}, success 200); DescribeSlotResponse.valueElicitationSetting targets the same SlotValueElicitationSetting shape. Recorded behavior only: a read-back of the slot returns the SAME defaulted prompt specification AWS stored at create time — messageSelectionStrategy "Random" and the full Initial/Retry1/Retry2 promptAttemptsSpecification described above.
Emulator: answers 200 with both members absent. Replay verdict `mismatch`, same two `missing` details, divergence key `service=lexv2-models|DescribeSlot|status=200->200|keys=$.valueElicitationSetting.promptSpecification.messageSelectionStrategy,$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification`. This is the read path confirming that the defaults were never applied when the slot was stored, rather than being dropped only by the CreateSlot serializer.

Why: clients read the prompt specification back to discover the effective runtime behavior of a slot. Against AWS, `response['valueElicitationSetting']['promptSpecification']['promptAttemptsSpecification']['Initial']` and `['messageSelectionStrategy']` are always present; against the emulator they KeyError / are None, so console-like tooling and config exporters that render or diff per-attempt audio, DTMF and text timeouts show nothing configured. A read-modify-write loop (DescribeSlot -> edit -> UpdateSlot) silently drops the defaults AWS would have preserved, and infrastructure tools that diff desired against actual state report a permanent drift because the members they expect never appear.

Fix: the slot's prompt-specification defaulting in the lexmodelsv2 crate — the CreateSlot handler's state write (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_slot.rs) should normalize each PromptSpecification it stores by filling messageSelectionStrategy with the service default and synthesizing one promptAttemptsSpecification entry per attempt (Initial plus Retry1..RetryN for maxRetries) with the documented default audio/DTMF/text input settings, leaving any member the request supplied untouched; the read-back handlers (describe_slot.rs, and the sibling slot-reporting operations such as UpdateSlot/ListSlots) then report it off that stored record. Defaulting at serialization time only would leave the stored state wrong and the two paths inconsistent.

Traces: lex-models-v2-bot-locale-slot-pipeline#task#8 (CreateSlot, representative; same cluster also lex-models-v2-bot-version-alias-lifecycle#task#9), lex-models-v2-bot-locale-slot-pipeline#task#9 (DescribeSlot).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, score_pct=60.0).

Cause 1 — fixture `lex-models-v2-bot-locale-slot-pipeline`, step `task#8`, operation CreateSlot, service lexv2-models:
  verdict: mismatch
  expected_status: 200   actual_status: 200
  detail:
    $.valueElicitationSetting.promptSpecification.messageSelectionStrategy: missing (expected 'Random', got None)
    $.valueElicitationSetting.promptSpecification.promptAttemptsSpecification: missing (expected {'Initial': {'allowInterrupt': True, 'allowedInputTypes': {'allowAudioInput': True, 'allowDTMFInput': True}, 'audioAndDTMFInputSpecification': {'audioSpecification': {'endTimeoutMs': 640, 'maxLengthMs': 15000}, 'dtmfSpecification': {'deletionCharacter': '*', 'endCharacter': '#', 'endTimeoutMs': 5000, 'maxLength': 513}, 'startTimeoutMs': 4000}, 'textInputSpecification': {'startTimeoutMs': 30000}}, 'Retry1': {...same...}, 'Retry2': {...same...}}, got None)
  divergence_key: service=lexv2-models|CreateSlot|status=200->200|keys=$.valueElicitationSetting.promptSpecification.messageSelectionStrategy,$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification
  affected (2): lex-models-v2-bot-locale-slot-pipeline#task#8, lex-models-v2-bot-version-alias-lifecycle#task#9
  stub_path: (empty)   owned_by: (empty)

Cause 2 — fixture `lex-models-v2-bot-locale-slot-pipeline`, step `task#9`, operation DescribeSlot, service lexv2-models:
  verdict: mismatch
  expected_status: 200   actual_status: 200
  detail: same two `missing` entries as above
  divergence_key: service=lexv2-models|DescribeSlot|status=200->200|keys=$.valueElicitationSetting.promptSpecification.messageSelectionStrategy,$.valueElicitationSetting.promptSpecification.promptAttemptsSpecification
  affected (1): lex-models-v2-bot-locale-slot-pipeline#task#9
  stub_path: (empty)   owned_by: (empty)

Expected (AWS, recorded): 200 whose valueElicitationSetting.promptSpecification contains messageSelectionStrategy = "Random" and a promptAttemptsSpecification map keyed Initial/Retry1/Retry2 with the default allowInterrupt / allowedInputTypes / audio+DTMF / text-input settings listed above.
Actual (emulator): 200 whose promptSpecification contains neither member.

Model basis: PromptSpecification members messageSelectionStrategy -> MessageSelectionStrategy (enum Random|Ordered) and promptAttemptsSpecification -> PromptAttemptsSpecificationMap (key PromptAttempt enum, value PromptAttemptSpecification); neither @required nor @default, so the defaulting is service behavior evidenced by the recording, not a model constraint. CreateSlot/DescribeSlot both bind valueElicitationSetting to SlotValueElicitationSetting.

## Steps to Reproduce
1. Boot the emulator from this checkout and confirm the lexv2-models (com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2) endpoint is serving.
2. CreateBot, CreateBotLocale (DRAFT + a locale), then CreateIntent so there is an intent to hang a slot off.
3. Call CreateSlot with a valueElicitationSetting whose promptSpecification supplies ONLY the required members, e.g. PUT /bots/{botId}/botversions/DRAFT/botlocales/{localeId}/intents/{intentId}/slots with valueElicitationSetting = {"slotConstraint": "Required", "promptSpecification": {"maxRetries": 2, "messageGroups": [{"message": {"plainTextMessage": {"value": "Which size?"}}}]}}.
4. Inspect the raw 200 body: the emulator's valueElicitationSetting.promptSpecification has no `messageSelectionStrategy` and no `promptAttemptsSpecification`. AWS returns messageSelectionStrategy "Random" and a promptAttemptsSpecification with Initial/Retry1/Retry2 entries (one per attempt for maxRetries=2) carrying the default audio (endTimeoutMs 640, maxLengthMs 15000), DTMF (deletionCharacter "*", endCharacter "#", endTimeoutMs 5000, maxLength 513), startTimeoutMs 4000 and text (startTimeoutMs 30000) settings.
5. Call DescribeSlot on the slot just created and inspect the 200 body — the same two members are still absent, showing the defaults were never stored.
6. Equivalently, replay fixture `lex-models-v2-bot-locale-slot-pipeline` steps `task#8` and `task#9` (and `lex-models-v2-bot-version-alias-lifecycle` step `task#9`) from trace set simparity-wire-aws-v1.
