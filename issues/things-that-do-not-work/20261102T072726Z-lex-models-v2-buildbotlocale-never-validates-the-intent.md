Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-models.build-rejects-required-slot-without-priority
Issue-Shape-Id: com.amazonaws.lexmodelsv2#LexModelBuildingServiceV2
Issue-Key: lexv2-models.build-rejects-required-slot-without-priority|run#11|AssertionError|assert failed["botLocaleStatus"] == "Failed", (
Issue-Commit: 88dedb85e694d97545027b37e695bddb9d3861f2

# lex-models-v2: BuildBotLocale never validates the intent graph — a required slot with no slotPriorities entry builds to 'Built' instead of settling at 'Failed' with the slot-priority failureReason

> _Note: this issue was identified against the code base at commit `88dedb85e694d97545027b37e695bddb9d3861f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BuildBotLocale

## Description
Cause 1 of 1: the emulator's BuildBotLocale is a status-flipper, not a build. It moves the locale Creating/NotBuilt -> Building -> Built without ever validating the locale's intents as a whole, so the one validation real Lex V2 performs only at build time — that every Required/Optional slot of an intent is ordered by an entry in that intent's slotPriorities — is enforced nowhere in the emulator.

AWS: Smithy (com.amazonaws.lexmodelsv2) models BuildBotLocale as POST /bots/{botId}/botversions/{botVersion}/botlocales/{localeId} returning HTTP 202 with botLocaleStatus: BotLocaleStatus; DescribeBotLocale returns botLocaleStatus plus failureReasons (FailureReasons, a list of FailureReason). BotLocaleStatus enumerates Creating, Building, Built, ReadyExpressTesting, Failed, Deleting, NotBuilt, Importing, Processing — i.e. 'Failed' with a populated failureReasons list is the modeled outcome of a build that cannot complete. The model itself places no constraint on slot ordering (CreateSlot legitimately returns 200 and does NOT add the slot to the intent's slotPriorities), so this is enforced by the asynchronous build only; recorded behavior for that build: the locale settles at botLocaleStatus 'Failed' and DescribeBotLocale reports exactly one failureReason whose fixed text is "Slot ids [<slotIds>] in intent <intentName> don't define a slot priority. Update the intent to add a priority to these slots." — the fixed text is everything outside the two substitutions; the variable parts are the comma-separated list of unordered slot ids/names taken from the intent's slots minus its slotPriorities, and the intent's intentName taken from state (in the recorded run those values were [PizzaSize] and OrderPizza, which fill the template and are not part of it). Adding the priority via UpdateIntent(slotPriorities=[{priority, slotId}]) and nothing else makes the same locale build to 'Built'.

Emulator: after CreateSlotType + CreateIntent + CreateSlot(valueElicitationSetting.slotConstraint='Required') with the intent's slotPriorities empty, BuildBotLocale is accepted with botLocaleStatus 'Building' (correct) and the locale then settles at botLocaleStatus 'Built' with no failureReasons. Replay assertion: "AssertionError: a build with an unprioritised required slot settled at 'Built', expected 'Failed'". Because the build never inspects the intents, no failureReason is ever produced for any cause, and the Failed terminal state appears to be unreachable.

Why: a client uses the build as its validation gate. A bot that real Lex V2 refuses to build is reported Built by the emulator, so the misconfiguration is never surfaced; code paths that poll for 'Failed' and read failureReasons to show the operator which slot to prioritise get a success they must not get, and any test or deployment pipeline that asserts an invalid locale fails to build passes against the emulator and breaks against AWS. Conversely nothing in the emulator can ever exercise the Failed/failureReasons branch.

Fix: the BuildBotLocale handler and the bot-locale build state machine for this service (services/service-com-amazonaws-lexmodelsv2-lexmodelbuildingservicev2 — the build op's handler plus the locale state it transitions). The build transition must validate the locale's intents against stored slot state before settling: for each intent, compute the slots whose slotConstraint is Required (or Optional) that have no matching entry in the intent's slotPriorities; if any exist, settle botLocaleStatus at 'Failed' and record the failureReason above on the locale so DescribeBotLocale serializes it in failureReasons, and clear failureReasons on a subsequent successful build.

Related open issues (do NOT cover this): 20261031T180911Z-lex-models-v2-createslot-stub.md and 20261031T192848Z-lexv2-models-createintent-serializes-absent-optional-context-lists-as-empty.md concern CreateSlot/CreateIntent, not the build-time validation; the check reached the build, so this is a distinct defect in BuildBotLocale.

Traces: lex-models-v2.build-rejects-required-slot-without-priority (mismatch at check line 202).

## Evidence
Expected (real AWS / recorded): BuildBotLocale -> 202 {botLocaleStatus: "Building"}; polling DescribeBotLocale settles at {"botLocaleStatus": "Failed", "failureReasons": ["Slot ids [PizzaSize] in intent OrderPizza don't define a slot priority. Update the intent to add a priority to these slots."]} — fixed text as quoted, with "[PizzaSize]" the request/state-derived list of unordered slot ids and "OrderPizza" the intent's intentName. After UpdateIntent(slotPriorities=[{priority:1, slotId:<slot>}]) the next BuildBotLocale settles at {"botLocaleStatus": "Built"} with no failureReasons.

Actual (emulator): BuildBotLocale -> 202 {botLocaleStatus: "Building"} (correct), then DescribeBotLocale settles at {"botLocaleStatus": "Built"} with no failureReasons, despite the intent having a Required slot absent from slotPriorities (the check first asserts DescribeIntent returns no slotPriorities, which held). Replay: AssertionError: a build with an unprioritised required slot settled at 'Built', expected 'Failed'.

Model citations: Smithy BuildBotLocale http {POST /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}, code 202}, output member botLocaleStatus -> BotLocaleStatus; DescribeBotLocale output members botLocaleStatus -> BotLocaleStatus and failureReasons -> FailureReasons (list of FailureReason); BotLocaleStatus values = [Creating, Building, Built, ReadyExpressTesting, Failed, Deleting, NotBuilt, Importing, Processing].

## Steps to Reproduce
1. CreateBot (dataPrivacy.childDirected=false, idleSessionTTLInSeconds=300, botType=Bot) with an assumable lexv2 role; wait for botStatus Available.
2. CreateBotLocale(botId, botVersion="DRAFT", localeId="en_US", nluIntentConfidenceThreshold=0.40); wait until botLocaleStatus leaves Creating.
3. CreateSlotType(slotTypeName="PizzaSize", one slotTypeValue with a synonym, valueSelectionSetting.resolutionStrategy=TopResolution).
4. CreateIntent(intentName="OrderPizza", sampleUtterances=["I want to order a pizza", "I want a {PizzaSize} pizza"]).
5. CreateSlot(intentId, slotName="PizzaSize", slotTypeId, valueElicitationSetting.slotConstraint="Required" with a promptSpecification). Confirm DescribeIntent returns no slotPriorities.
6. BuildBotLocale(botId, "DRAFT", "en_US") -> 202 botLocaleStatus "Building".
7. Poll DescribeBotLocale until botLocaleStatus is outside (Creating, Building, Processing, ReadyExpressTesting).
   Expected: "Failed" with failureReasons naming the unprioritised slot and the intent. Observed: "Built" with no failureReasons.
8. Positive control: UpdateIntent(..., slotPriorities=[{priority:1, slotId}]) and BuildBotLocale again -> must settle "Built" (this half must keep working after the fix).
