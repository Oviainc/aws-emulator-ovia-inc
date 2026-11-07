Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent lexv2-runtime.recognize-text-elicits-missing-required-slot lexv2-runtime.session-state-persists-across-turns lexv2-runtime.unmatched-utterance-returns-fallback-intent lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text lexv2-runtime.deleted-session-state-is-gone-and-get-session-rejected
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent|run#9|AssertionError|assert intent["name"] == _INTENT, (
Issue-Key: lexv2-runtime.recognize-text-elicits-missing-required-slot|run#7|AssertionError|assert action["type"] == "ElicitSlot", (
Issue-Key: lexv2-runtime.session-state-persists-across-turns|run#5|AssertionError|assert first["sessionState"]["dialogAction"]["type"] == "ElicitSlot", (
Issue-Key: lexv2-runtime.unmatched-utterance-returns-fallback-intent|run#8|KeyError|assert state["intent"]["slots"] == {}, (
Issue-Key: lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text|run#13|KeyError|assert state["intent"]["state"] == "Fulfilled", (
Issue-Key: lexv2-runtime.deleted-session-state-is-gone-and-get-session-rejected|run#5|KeyError|origin = first["sessionState"]["originatingRequestId"]
Issue-Commit: 3e82efa9dc9c0e3c4a597a967c25a995e0e693a6

# lexv2-runtime RecognizeText/RecognizeUtterance run no NLU or slot dialog against the built locale, and the SessionState they serialize omits intent.slots, intent.state and originatingRequestId (4 causes)

> _Note: this issue was identified against the code base at commit `3e82efa9dc9c0e3c4a597a967c25a995e0e693a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecognizeText

## Description
The Lex V2 runtime data plane answers every turn from a canned, bot-independent session turn. services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0/src/ops/recognize_text.rs calls state.lock().advance_turn(key, now, input.session_state, input.request_attributes) and never passes or reads input.text; src/state.rs documents the choice (FALLBACK_INTENT: "no intent can be matched from an utterance"; advance_turn: "this emulator holds no bot locale to match it against ... so it is not interpreted into an intent of its own and is not passed here"). Four distinct divergences follow from that gap, and all four must be closed before this issue is submitted. All were exercised against a bot whose DRAFT en_US locale (nluIntentConfidenceThreshold 0.40) was given a Flavor slot type (chocolate/vanilla/strawberry), an OrderShake intent with sample utterances "i would like a {Flavor} shake" / "order a shake" / "i would like a shake", a Required Flavor slot with the elicitation prompt "What flavor would you like?" and a closingResponse "Your shake is on the way.", then built with BuildBotLocale to botLocaleStatus Built.

Related open issue: 20261104T180144Z-lexv2-models-lexv2-runtime-lexv2-runtime-recognizetext-has-no-nlu-or-dialog.md records the same engine gap from the lexv2-models__lexv2-runtime checks. It does NOT cover the causes below: causes 3 and 4 (the omitted Intent.slots/Intent.state members and the never-minted SessionState.originatingRequestId) are not in it at all, and it carries none of the lexv2-runtime check ids listed here.

Cause 1 of 4 — no intent of the built locale is ever matched and no interpretation is ever scored.
AWS: RecognizeText "interprets the user input using the machine learning model that it build for the bot" (Smithy lexruntimev2#RecognizeText doc). The 200 response carries interpretations (Interpretations, len 0..5) ordered by confidence; each Interpretation has intent (Intent, required member name) and an optional nluConfidence (ConfidenceScore.score, Double 0.00..1.00). An utterance matching an authored sample utterance of a built locale returns that intent as the top interpretation with a score at or above the locale's nluIntentConfidenceThreshold; an utterance nothing matches returns FallbackIntent on top AND still reports the bot's own intents as scored alternatives with nluConfidence.score below that threshold.
Emulator: every turn, for every text, reports exactly one interpretation whose intent is FallbackIntent and which carries no nluConfidence at all. "AssertionError: recognized intent 'FallbackIntent', expected 'OrderShake'" for the sample utterance "i would like a chocolate shake"; on the unrelated utterance "what is the weather in reykjavik tomorrow afternoon" there is no scored alternative to compare against the threshold, so the threshold cannot be observed to do anything.
Why: a client cannot test any conversation it authored. Every turn is misrouted to the fallback path, so intent dispatch, analytics on intent name and confidence gating against nluIntentConfidenceThreshold are exercised against an intent the bot does not declare and never against its own; and because nothing is ever scored, a fallback caused by a low score is indistinguishable from a bot that matches nothing.
Fix: the turn component in the runtime crate's state (advance_turn) must take the utterance text and resolve the addressed bot locale built through lexv2-models (botId + botAliasId + localeId), match the locale's intents by their sample utterances, and report the match as the top interpretation with a score, emitting FallbackIntent plus scored alternatives only when nothing clears the locale's threshold.
Traces: lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent, lexv2-runtime.unmatched-utterance-returns-fallback-intent, lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text.

Cause 2 of 4 — a Required slot with an authored prompt is never elicited, no slot value is ever filled or resolved, and no authored message is ever returned.
AWS: Smithy lexruntimev2#DialogAction.type is required and enumerates Close/ConfirmIntent/Delegate/ElicitIntent/ElicitSlot/None, with slotToElicit "the name of the slot that should be elicited from the user"; Intent.state enumerates InProgress ("the bot is in the middle of eliciting the slot values that are necessary to fulfill the intent"), ReadyForFulfillment and Fulfilled. A turn that matches an intent but names no value for its Required slot returns dialogAction.type ElicitSlot with slotToElicit = that slot's name, intent.state InProgress, and messages holding the slot's valueElicitationSetting.promptSpecification text; the answering turn fills Slot.value (Value.interpretedValue required, originalValue "the part of the user's response ... that Amazon Lex V2 determines is relevant", resolvedValues "possible resolutions for the user input", first matching interpretedValue), leaves the intent Fulfilled, closes the dialog and returns the intent's closingResponse message. The fill is session state: GetSession on the same four labels reports the same sessionState.intent.slots entry and the same open elicitation.
Emulator: the first turn of every conversation returns dialogAction {'type': 'Close'} with no slotToElicit member — "AssertionError: dialogAction was {'type': 'Close'}, expected type 'ElicitSlot' -- the required slot is unset" and "turn one left dialogAction {'type': 'Close'}, expected 'ElicitSlot' -- there is nothing for turn two to continue". No slots entry is ever written (state.rs builds the turn's Intent with slots: None), so no answering text resolves anything, and no authored prompt or closing message is returned.
Why: the conversation is one turn long and always terminal. A client driving a multi-turn dialog off dialogAction.type/slotToElicit — the documented way to know what to ask next — sees a closed dialog on turn one, never collects the slot, cannot read a filled slot back from the session on the following turn, and cannot verify its own prompt or closing text.
Fix: the same turn component must run the matched intent's slot-priority dialog loop — emit ElicitSlot with slotToElicit and the slot's promptSpecification messages while a Required slot is unfilled, hold intent.state at InProgress, resolve answers against the slot type into Value{originalValue, interpretedValue, resolvedValues}, store them on the session's intent.slots so GetSession returns them, and only then close with the intent's closingResponse.
Traces: lexv2-runtime.recognize-text-elicits-missing-required-slot, lexv2-runtime.session-state-persists-across-turns, lexv2-runtime.deleted-session-state-is-gone-and-get-session-rejected, lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text.

Cause 3 of 4 — the Intent the runtime serializes carries neither a slots map nor a state, so those members are absent rather than empty/known.
AWS: Smithy lexruntimev2#Intent.slots is "a map of all of the slots for the intent. The name of the slot maps to the value of the slot. If a slot has not been filled, the value is null", and Intent.state is the fulfillment state of the turn. A RecognizeText/RecognizeUtterance turn reports both: an intent with nothing filled reports slots as an empty map ({}), and a closed turn reports a state (Fulfilled for a turn the bot completed).
Emulator: both members are omitted from the serialized Intent — reading them raises "KeyError: 'slots'" (on the fallback turn, where AWS reports {}) and "KeyError: 'state'" (on the RecognizeUtterance turn). src/state.rs builds the turn's intent as Intent { name: FALLBACK_INTENT, slots: None, state: None, confirmation_state: None }.
Why: a client that indexes sessionState.intent.slots or branches on sessionState.intent.state — the two members every Lex dialog loop reads — crashes with a missing-key error instead of seeing an empty map and a state, so even the fallback path cannot be consumed by code written against AWS.
Fix: the turn component must populate Intent.slots (an empty map when nothing is filled, not an absent member) and Intent.state on every turn it returns, for the fallback intent as well as a matched one.
Traces: lexv2-runtime.unmatched-utterance-returns-fallback-intent (KeyError: 'slots'), lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text (KeyError: 'state').

Cause 4 of 4 — SessionState.originatingRequestId is never minted by the service; it is only echoed back when the client supplies one.
AWS: Smithy lexruntimev2#SessionState.originatingRequestId is "a unique identifier for a specific request" (NonEmptyString, len 1..). The service mints it and reports it in the sessionState of every turn and of GetSession; the turns of one conversation share it, and a conversation started after DeleteSession on the same session id reports a different one.
Emulator: the member is absent from the response — "KeyError: 'originatingRequestId'" reading first['sessionState']['originatingRequestId'] on the opening RecognizeText turn. src/state.rs merge_session_state sets self.session_state.originating_request_id only from a caller-supplied value, and nothing in the crate ever generates one, so a client that did not send it never gets one back.
Why: originatingRequestId is how a client correlates the turns of one conversation and how it tells a resumed conversation from a new one after DeleteSession; absent, that correlation is impossible and code reading the member faults.
Fix: the runtime's session state must mint an originatingRequestId when a session comes into existence and report it on every sessionState it serializes (RecognizeText, RecognizeUtterance, PutSession, GetSession), keeping it stable for the life of the session and fresh for a session created after a delete.
Traces: lexv2-runtime.deleted-session-state-is-gone-and-get-session-rejected (the failure point), lexv2-runtime.put-session-seeds-dialog-state-for-the-next-turn (asserts the turn after a seed reports PutSession's id).

## Evidence
Suite: lexv2-runtime 0/10 (0%), 10 failed. The six checks clustered here:

1) [mismatch] lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent
   AssertionError: recognized intent 'FallbackIntent', expected 'OrderShake'
   Expected (AWS) for text "i would like a chocolate shake" on a Built en_US locale: sessionState.intent.name OrderShake, intent.state Fulfilled, intent.slots["Flavor"].value.interpretedValue "chocolate" with resolvedValues ["chocolate"], dialogAction.type Close, messages ["Your shake is on the way."], interpretations[0].intent.name OrderShake with nluConfidence.score >= 0.40.
   Actual (emulator): interpretations == [FallbackIntent] with no nluConfidence; no slots; dialogAction Close.

2) [mismatch] lexv2-runtime.recognize-text-elicits-missing-required-slot
   AssertionError: dialogAction was {'type': 'Close'}, expected type 'ElicitSlot' -- the required slot is unset
   Expected (AWS) for "order a shake": dialogAction.type ElicitSlot, dialogAction.slotToElicit "Flavor", intent.state InProgress, messages ["What flavor would you like?"].
   Actual: {'type': 'Close'}, no slotToElicit member, no messages.

3) [mismatch] lexv2-runtime.session-state-persists-across-turns
   AssertionError: turn one left dialogAction {'type': 'Close'}, expected 'ElicitSlot' -- there is nothing for turn two to continue

4) [error] lexv2-runtime.unmatched-utterance-returns-fallback-intent
   KeyError: 'slots' at assert state["intent"]["slots"] == {}
   Expected (AWS) for "what is the weather in reykjavik tomorrow afternoon": intent.name FallbackIntent, intent.slots {} (an empty map, present), dialogAction.type Close, interpretations[0] FallbackIntent plus exactly one scored alternative (the bot's OrderShake) whose nluConfidence.score < 0.40.
   Actual: intent has no slots member; the single interpretation is unscored.

5) [error] lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text
   KeyError: 'state' at assert state["intent"]["state"] == "Fulfilled" (state decoded from the gzip+base64 x-amz-lex-session-state header)
   Expected (AWS) for "i would like a strawberry shake": decoded dialogAction.type Close, intent.state Fulfilled, intent.slots["Flavor"].value.resolvedValues ["strawberry"], decoded messages ["Your shake is on the way."], decoded interpretations[0].intent.name OrderShake, and the same intent object from RecognizeText for the same sentence.
   Actual: the decoded Intent has no state and no slots member and names FallbackIntent.

6) [error] lexv2-runtime.deleted-session-state-is-gone-and-get-session-rejected
   KeyError: 'originatingRequestId' at origin = first["sessionState"]["originatingRequestId"]
   Expected (AWS): the opening turn's sessionState carries an originatingRequestId and dialogAction ElicitSlot/slotToElicit "Flavor"; after DeleteSession, GetSession is 404 ResourceNotFoundException "The session for user <sessionId> doesn\u2019t exist or has expired." (fixed text around the sessionId from the request, apostrophe U+2019), and reusing the session id starts a new conversation with empty sessionAttributes and a DIFFERENT originatingRequestId.
   Actual: no originatingRequestId member at all, and the opening turn is Close.

Source evidence for the shared gap: ops/recognize_text.rs:236-239 passes only (key, now, session_state, request_attributes) to advance_turn — input.text is never used; state.rs:25-30 (FALLBACK_INTENT, "no intent can be matched from an utterance"), state.rs:356-361 (Intent { name: FALLBACK_INTENT, slots: None, state: None, .. }), state.rs:363-370 (dialog closes unless the caller set the next step), state.rs:569-571 (originating_request_id stored only from a caller-supplied value).

## Steps to Reproduce
1. lexv2-models: CreateBot (botType Bot, dataPrivacy.childDirected false, idleSessionTTLInSeconds 300) with an IAM role lexv2.amazonaws.com can assume; wait for botStatus Available.
2. CreateBotLocale botVersion DRAFT, localeId en_US, nluIntentConfidenceThreshold 0.40; wait for it to leave Creating.
3. CreateSlotType "Flavor" with sample values chocolate/vanilla/strawberry; CreateIntent "OrderShake" with sampleUtterances ["i would like a {Flavor} shake", "order a shake", "i would like a shake"] and closingResponse "Your shake is on the way."; CreateSlot "Flavor" Required with promptSpecification "What flavor would you like?"; set slotPriorities.
4. BuildBotLocale and wait for botLocaleStatus Built; create/locate the bot alias used for the runtime calls.
5. Cause 1/2: lexv2-runtime RecognizeText(botId, botAliasId, localeId=en_US, sessionId=<fresh>, text="i would like a chocolate shake"). Emulator: intent FallbackIntent, dialogAction Close, no slots, one unscored interpretation. AWS: OrderShake / Fulfilled / Flavor=chocolate / Close / closing message / scored interpretation >= 0.40.
6. Cause 2: in a fresh session, RecognizeText(text="order a shake"). Emulator: dialogAction {'type':'Close'}. AWS: ElicitSlot with slotToElicit "Flavor", intent.state InProgress, messages ["What flavor would you like?"]. Then RecognizeText(text="chocolate") in the same session and GetSession: AWS reports the filled slot; the emulator reports none.
7. Cause 3: RecognizeText(text="what is the weather in reykjavik tomorrow afternoon") and read sessionState.intent["slots"] — KeyError on the emulator, {} on AWS; RecognizeUtterance (requestContentType and responseContentType "text/plain; charset=utf-8", inputStream="i would like a strawberry shake") and read intent["state"] from the gzip+base64 x-amz-lex-session-state header — KeyError on the emulator, "Fulfilled" on AWS.
8. Cause 4: on any RecognizeText turn, read sessionState["originatingRequestId"] — absent on the emulator; present on AWS, stable across the turns of the session and different for a conversation started on the same session id after DeleteSession.
