Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.create-slot-type-rejects-duplicate-name
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models.create-slot-type-rejects-duplicate-name|run#8|AssertionError|assert message == (
Issue-Commit: 614aa2f240af017c8d67ce8cf3a870b05aeb16dc

# lex-models-v2 CreateSlotType duplicate-name ValidationException message is emulator-worded, not AWS's "Slot Type with name <name> already exists. Use another name."

> _Note: this issue was identified against the code base at commit `614aa2f240af017c8d67ce8cf3a870b05aeb16dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSlotType

## Description
Cause 1 of 1: CreateSlotType correctly refuses a second slot type whose name is already taken in the bot/version/locale with HTTP 400 ValidationException, but it writes its own message text instead of the message AWS returns for this refusal class. Failing check: lex-models-v2.create-slot-type-rejects-duplicate-name (the check's code and status assertions pass; only the message assertion fails).

AWS: Smithy (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/smithy-model/com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2.json): CreateSlotType is PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/slottypes, success 200, with modeled errors ConflictException(409), InternalServerException(500), PreconditionFailedException(412), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400) — "One of the input parameters in your request isn't valid." slotTypeName is documented as "A slot type name must be unique", so re-using a name in the same locale is a ValidationException. The message text itself is recorded behavior only: the recording shows AWS answering 400 ValidationException with the message "Slot Type with name <slotTypeName> already exists. Use another name." Fixed text of that template: "Slot Type with name " ... " already exists. Use another name." — the ONLY variable part is the slot type name copied verbatim from the request's slotTypeName member ("PizzaSize" in the recorded run; the bot id, version and locale do not appear in AWS's message at all).

Emulator: the same call answers 400 ValidationException with the message "A slot type with the name <slotTypeName> already exists in locale <localeId> of bot <botId> version <botVersion>. Choose a different name and try again." (observed: 'A slot type with the name PizzaSize already exists in locale en_US of bot S2gHCwPblp version DRAFT. Choose a different name and try again.'). Different fixed text ("A slot type with the name" / "Choose a different name and try again." instead of "Slot Type with name" / "Use another name.") and extra interpolated locale/botId/botVersion values AWS's message never carries. The rest of the behavior the check asserts is right: the refused create leaves the existing slot type's values and resolutionStrategy untouched, the locale lists only the original slot type id, and a different name in the same locale is accepted.

Why: a client that matches on the refusal message — a user-facing error surface, a log/alert rule, a test that asserts the AWS wording, or retry logic that distinguishes "name taken" from other ValidationExceptions (all ValidationException, so the code alone cannot discriminate) — fails against the emulator while passing against AWS. The extra botId/botVersion/locale text also leaks identifiers into a message whose shape clients may parse, so message-driven branching behaves differently between the emulator and the real service.

Fix: the duplicate-name refusal branch of the CreateSlotType handler — services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_slot_type.rs, the store.slot_type_name_taken(...) guard that formats the ValidationException message. The status (400) and error code (ValidationException) are already correct; only the message template must be AWS's, with the request's slotTypeName as its single substitution. Unrelated open issues on this operation (20261030T035641Z-lexv2-models-createslottype-response-content-type-is-application-json-not.md and 20261030T011107Z-lexv2-models-lex-models-v2-responses-carry-content-type-application.md) concern the success response's Content-Type header, not this refusal message, so neither covers this cause.

Traces: lex-models-v2.create-slot-type-rejects-duplicate-name.

## Evidence
Failing behavioral check lex-models-v2.create-slot-type-rejects-duplicate-name, verdict=mismatch, at check_lex_models_v2_create_slot_type_rejects_duplicate_name.py line 179:

  AssertionError: got message='A slot type with the name PizzaSize already exists in locale en_US of bot S2gHCwPblp version DRAFT. Choose a different name and try again.'

Expected (AWS, per the check's recorded reference): code 'ValidationException' (HTTP 400) with message exactly 'Slot Type with name PizzaSize already exists. Use another name.' — template 'Slot Type with name {slotTypeName} already exists. Use another name.', fixed text everywhere except {slotTypeName}, taken from the request.
Actual (emulator): code 'ValidationException' (HTTP 400) — code and status match — with message template 'A slot type with the name {slotTypeName} already exists in locale {localeId} of bot {botId} version {botVersion}. Choose a different name and try again.'
Source of the divergence: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_slot_type.rs lines 280-295 (slot_type_name_taken guard -> ValidationException message format!).
The check's remaining assertions (unchanged existing slot type values/strategy, ListSlotTypes showing only the first id, acceptance of a different name) are not reached but are unaffected by this message defect.

## Steps to Reproduce
1. Boot the emulator from the checkout and run the behavioral suite (simparity-behavioral-aws-v1), check lex-models-v2.create-slot-type-rejects-duplicate-name.
2. Equivalent manual repro against the emulator's lexv2-models endpoint:
   a. CreateBot (with an IAM role, dataPrivacy.childDirected=false, idleSessionTTLInSeconds=300, botType=Bot) and wait for botStatus 'Available'.
   b. CreateBotLocale(botId, botVersion='DRAFT', localeId='en_US', nluIntentConfidenceThreshold=0.40) and wait for it to leave 'Creating'.
   c. CreateSlotType(botId, botVersion='DRAFT', localeId='en_US', slotTypeName='PizzaSize', slotTypeValues=[{sampleValue:{value:'large'}},{sampleValue:{value:'small'}}], valueSelectionSetting={resolutionStrategy:'TopResolution'}) — succeeds, note slotTypeId.
   d. CreateSlotType again with the SAME slotTypeName='PizzaSize' in the same bot/version/locale (any values, e.g. [{sampleValue:{value:'gigantic'}}], resolutionStrategy='OriginalValue').
3. Observe the 400 ValidationException: its message is 'A slot type with the name PizzaSize already exists in locale en_US of bot <botId> version DRAFT. Choose a different name and try again.' AWS answers 'Slot Type with name PizzaSize already exists. Use another name.'
