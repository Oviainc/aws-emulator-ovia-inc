Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.build-rejects-locale-with-no-utterances lexv2-models.create-slot-rejects-unknown-slot-type
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models.build-rejects-locale-with-no-utterances|run#9|AssertionError|assert failed["botLocaleStatus"] == "Failed", (
Issue-Key: lexv2-models.create-slot-rejects-unknown-slot-type|run#10|AssertionError|assert code == "PreconditionFailedException", (
Issue-Commit: 30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481

# lex-models-v2: 2 net-new root causes — BuildBotLocale always settles Built (a locale with no utterances never reaches Failed with failureReasons), and CreateSlot never refuses an unknown slotTypeId with 412 PreconditionFailedException

> _Note: this issue was identified against the code base at commit `30fa5e7e24c2e7ff08a1ee60d32c6c6e65abb481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BuildBotLocale

## Description
Two independent root causes in the lex-models-v2 (Lex Model Building Service V2) emulator, both observed by replaying the behavioral suite. Each must be closed before this issue is submitted.

Cause 1 of 2: BuildBotLocale/DescribeBotLocale have no build-validation outcome — every submitted build settles on 'Built', so a locale that AWS fails the build for is indistinguishable from one that builds.
Failing check id: lex-models-v2.build-rejects-locale-with-no-utterances
AWS: BuildBotLocale is accepted asynchronously (Smithy: BuildBotLocale http POST /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}, success code 202, output member botLocaleStatus -> BotLocaleStatus) and the outcome is only observable through DescribeBotLocale. Smithy: BotLocaleStatus enum values are Creating | Building | Built | ReadyExpressTesting | Failed | Deleting | NotBuilt | Importing | Processing, i.e. 'Failed' is a modeled terminal build outcome; DescribeBotLocale output carries failureReasons (FailureReasons, a list of FailureReason) and lastBuildSubmittedDateTime (Timestamp) precisely so a caller can read WHY a build failed. The model does not state the buildability rule itself, so for the rule it is recorded behavior only: real AWS refuses to build a locale that has no custom intent with at least one valid sample utterance, settling that locale at botLocaleStatus 'Failed' and reporting exactly one failure reason whose fixed text is "The locale '<localeId>' doesn't have any utterances. A locale must have at least one custom intent with a valid utterance. Add a custom intent and try your request again." — the only variable part is the locale id taken from the request (here en_US); the rest of the sentence is fixed text. lastBuildSubmittedDateTime is still present on the failed locale (the build DID run). Supplying the missing utterance (UpdateIntent with one sampleUtterances entry) and rebuilding the same locale then settles it at 'Built' with no failureReasons.
Emulator: the build is recorded unconditionally and the clock settles it to Built regardless of locale content. Replay observed DescribeBotLocale report botLocaleStatus 'Built' for a locale whose single custom intent (OrderPizza) had NO sample utterances: "AssertionError: a build with no utterances settled at 'Built', expected 'Failed'". No failureReasons are ever produced, and there is no code path that can yield the 'Failed' status at all.
Why: a client that builds a locale and polls DescribeBotLocale for the terminal status is told the bot built successfully when AWS would have failed the build, and it never sees the failureReasons text that tells it what to fix. Build pipelines, CI gates and consoles that branch on Built vs Failed take the success branch on a bot that cannot serve, and the subsequent runtime failure surfaces far from its cause.
Fix: the BuildBotLocale handler plus the locale build state it drives — the build submission must evaluate the locale's intents/slot types for buildability and record a terminal 'Failed' with the single modeled failure reason instead of always transitioning to 'Built'; DescribeBotLocale must then serialize failureReasons for that locale and keep lastBuildSubmittedDateTime set.
Traces: lex-models-v2.build-rejects-locale-with-no-utterances. Handler/state citations: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/build_bot_locale.rs lines 179-211 (the only refusal is a missing locale -> PreconditionFailedException; otherwise locale.submit_build(now) is called unconditionally and the status is reported straight back) and the settle logic behind crate::state::BUILD_SETTLES_AFTER / submit_build in services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/state.rs.

Cause 2 of 2: CreateSlot does not refuse a slotTypeId that names nothing in the locale — the referential precondition is absent, so the call cannot answer 412 PreconditionFailedException.
Failing check id: lex-models-v2.create-slot-rejects-unknown-slot-type
AWS: Smithy: CreateSlot http PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents/{intentId}/slots, success 200; input member slotTypeId -> BuiltInOrCustomSlotTypeId (@length 1..25, @pattern ^((AMAZON\.)[a-zA-Z_]+?|[0-9a-zA-Z]+)$); modeled errors include PreconditionFailedException (HTTP 412, "Your request couldn't be completed because one or more request fields aren't valid. Check the fields in your request and try again."), ConflictException 409, ValidationException 400. Recorded behavior for the rule: a CreateSlot whose slotTypeId is well-formed but names no slot type in that bot/version/locale is refused with HTTP 412, Error.Code 'PreconditionFailedException' and Error.Message "Failed to retrieve parent resource since it does not exist" — that message is entirely fixed text for this error class, carrying no request-derived value. The refusal creates nothing: a following ListSlots for the intent returns an empty slotSummaries. The identical request differing only in a slotTypeId that DOES exist is accepted, and the created slot is then visible in ListSlots and DescribeSlot reports back the slotTypeId it was given.
Emulator: CreateSlot answers HTTP 501 with Error.Code 'NotImplemented' and message "operation CreateSlot is scaffolded but not implemented in this emulator": "AssertionError: CreateSlot against an unknown slot type returned 'NotImplemented', expected 'PreconditionFailedException'". Note: the open issue 20261031T180911Z-lex-models-v2-createslot-stub asks for CreateSlot to be implemented, but it does NOT carry this check id in its Issue-Trace-Ids, so this referential-precondition requirement and its exact refusal are not covered by it — implementing a CreateSlot that happily accepts an unknown slotTypeId would close that issue and leave this check failing.
Why: a client gets 501 instead of the documented 412, so it cannot distinguish "you referenced a slot type that does not exist" from an emulator gap, and nothing in the locale is validated; once CreateSlot is implemented without this check, slots would be created pointing at non-existent slot types, and the divergence moves from a refusal to silent corruption that only shows up at build or runtime.
Fix: the CreateSlot handler and its validation — resolve slotTypeId against the slot types stored for the (botId, botVersion, localeId) triple (and the AMAZON.* built-ins) before any write, return PreconditionFailedException with the fixed message above, and persist nothing on that path; the accepted path must register the slot so ListSlots and DescribeSlot observe it.
Traces: lex-models-v2.create-slot-rejects-unknown-slot-type. Handler citation: services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2/src/ops/create_slot.rs (scaffold stub returning crate::scaffolded::Scaffolded::not_implemented).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replay, 1/13 passed.

Cause 1 — [mismatch] lex-models-v2.build-rejects-locale-with-no-utterances:
  File "check_lex_models_v2_build_rejects_locale_with_no_utterances.py", line 184, in run
    assert failed["botLocaleStatus"] == "Failed", (
  AssertionError: a build with no utterances settled at 'Built', expected 'Failed'
Expected (AWS): DescribeBotLocale -> botLocaleStatus 'Failed', failureReasons == ["The locale '<localeId>' doesn't have any utterances. A locale must have at least one custom intent with a valid utterance. Add a custom intent and try your request again."], lastBuildSubmittedDateTime present; after UpdateIntent adds one utterance, a rebuild settles 'Built' with no failureReasons.
Actual (emulator): botLocaleStatus 'Built', no failureReasons, for a locale whose only custom intent has zero sampleUtterances.

Cause 2 — [mismatch] lex-models-v2.create-slot-rejects-unknown-slot-type:
  File "check_lex_models_v2_create_slot_rejects_unknown_slot_type.py", line 189, in run
    assert code == "PreconditionFailedException", (
  AssertionError: CreateSlot against an unknown slot type returned 'NotImplemented', expected 'PreconditionFailedException'
Expected (AWS): HTTP 412, Error.Code 'PreconditionFailedException', Error.Message "Failed to retrieve parent resource since it does not exist"; ListSlots for the intent still empty; the same request with a real slotTypeId returns 200 and the slot is listed.
Actual (emulator): HTTP 501, Error.Code 'NotImplemented', Error.Message "operation CreateSlot is scaffolded but not implemented in this emulator".

## Steps to Reproduce
Cause 1:
1. Boot the lex-models-v2 emulator.
2. CreateBot (dataPrivacy.childDirected=false, idleSessionTTLInSeconds=300, botType=Bot) and wait for botStatus 'Available'.
3. CreateBotLocale botVersion=DRAFT localeId=en_US nluIntentConfidenceThreshold=0.40 and wait for it to leave 'Creating'.
4. CreateSlotType (PizzaSize) and CreateIntent (OrderPizza) with NO sampleUtterances; DescribeIntent confirms it has none.
5. BuildBotLocale botId/DRAFT/en_US (returns 202, botLocaleStatus 'Building'), then poll DescribeBotLocale until the status leaves Creating/Building/Processing/ReadyExpressTesting.
6. Observe botLocaleStatus 'Built' and no failureReasons; AWS reports 'Failed' with the single no-utterance failure reason.
7. UpdateIntent adding sampleUtterances=[{"utterance":"I want to order a pizza"}] and rebuild: AWS then settles 'Built' with no failureReasons.

Cause 2:
1. Boot the lex-models-v2 emulator; create a bot and an en_US DRAFT locale as above.
2. CreateSlotType PizzaSize (note its slotTypeId) and CreateIntent OrderPizza with one utterance (note its intentId).
3. Call CreateSlot with slotName='Doomed', slotTypeId='ZZZZZZZZZZ' (well-formed, names nothing in the locale) and a Required valueElicitationSetting with a promptSpecification.
4. Observe HTTP 501 NotImplemented; AWS answers HTTP 412 PreconditionFailedException "Failed to retrieve parent resource since it does not exist".
5. ListSlots for the intent must still return an empty slotSummaries.
6. Repeat step 3 with the real slotTypeId: it must return 200, ListSlots must then report ['PizzaSize'], and DescribeSlot must report that slotTypeId.
