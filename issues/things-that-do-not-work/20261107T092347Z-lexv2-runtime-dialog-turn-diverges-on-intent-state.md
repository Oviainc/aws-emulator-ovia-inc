Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent lexv2-runtime.recognize-text-rejects-an-unbuilt-locale lexv2-runtime.recognize-text-elicits-missing-required-slot lexv2-runtime.session-state-persists-across-turns lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent|run#10|AssertionError|assert intent["state"] == "Fulfilled", (
Issue-Key: lexv2-runtime.recognize-text-rejects-an-unbuilt-locale|run#19|AssertionError|assert r["sessionState"]["intent"]["state"] == "Fulfilled", (
Issue-Key: lexv2-runtime.recognize-text-elicits-missing-required-slot|run#12|KeyError|assert intent["slots"] == {_SLOT: None}, (
Issue-Key: lexv2-runtime.session-state-persists-across-turns|run#6|KeyError|origin = first["sessionState"]["originatingRequestId"]
Issue-Key: lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias|run#8|AssertionError|assert message == (
Issue-Key: lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text|run#10|AssertionError|assert audio == b"", (
Issue-Commit: d46c3419c1cd7253543b48c7e2be0670320596d1

# lexv2-runtime dialog turn diverges on Intent.state, Intent.slots, SessionState.originatingRequestId, the BotId/BotAliasId refusal message, and RecognizeUtterance's text audioStream (5 causes)

> _Note: this issue was identified against the code base at commit `d46c3419c1cd7253543b48c7e2be0670320596d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecognizeText

## Description
The Lex V2 runtime now interprets utterances against the built locale (the NLU/dialog engine from 20261104T180144Z-lexv2-models-lexv2-runtime-lexv2-runtime-recognizetext-has-no-nlu-or-dialog.md landed, and the four lexv2-models__lexv2-runtime checks pass), but five distinct divergences remain in what a turn reports and how it is refused. All five must be closed before this issue is submitted.

Related open issues, neither of which covers the causes below: 20261104T223017Z-lexv2-runtime-recognizetext-recognizeutterance-run-no-nlu-or.md (its causes 1 and 2 — the missing NLU and the missing slot dialog — are now served; its cause 3 asserts only that Intent.slots must be an EMPTY map on a FallbackIntent turn, not the stronger rule in Cause 2 here, and it carries the replay keys of other checks) and 20261104T223017Z-lexv2-runtime-fails-open-on-addressing-and-seeding.md (its causes 1 and 2 are now refused with the right CODE; the diverging MESSAGE in Cause 4 here is not described in it). Every cause below was exercised against a bot whose DRAFT en_US locale (nluIntentConfidenceThreshold 0.40) holds a custom Flavor slot type (chocolate/vanilla/strawberry), an OrderShake intent with sample utterances "i would like a {Flavor} shake" / "order a shake" / "i would like a shake", a Required Flavor slot with promptSpecification "What flavor would you like?" and an intentClosingSetting closingResponse "Your shake is on the way.", built to botLocaleStatus Built.

Cause 1 of 5 — a closed turn reports Intent.state ReadyForFulfillment where AWS reports Fulfilled.
AWS: Smithy lexruntimev2#Intent.state targets IntentState, values [Failed, Fulfilled, InProgress, ReadyForFulfillment, Waiting, FulfillmentInProgress]; the model documents "Fulfilled – The bot has completed fulfillment of the intent" and "ReadyForFulfillment – The bot has elicited all the slot values for the intent and is ready to fulfill the intent". Recorded behavior: for an intent with every required slot filled, no fulfillment code hook, and an authored intentClosingSetting closingResponse, the turn that closes the dialog reports dialogAction.type Close together with intent.state "Fulfilled" — the bot completed the intent itself and played its closing response. This holds for a single fully specified utterance and for the turn that answers an elicitation.
Emulator: that turn reports intent.state "ReadyForFulfillment" — "AssertionError: intent state was 'ReadyForFulfillment', expected 'Fulfilled'" and, after a build, "AssertionError: after the build the intent state was 'ReadyForFulfillment', expected 'Fulfilled'". state/nlu.rs hardcodes INTENT_STATE_READY_FOR_FULFILLMENT on the branch where no required slot is unfilled, and its module doc states that rule ("once every required slot has one the intent is ReadyForFulfillment and the dialog closes").
Why: Fulfilled vs ReadyForFulfillment is the branch every Lex client uses to decide whether the bot finished the intent or whether the CLIENT must still fulfil it. Against the emulator a completed order looks like a pending hand-off, so client code either fulfils twice or never treats any conversation as done.
Fix: the dialog-state decision in the runtime crate's turn component — the branch that closes the dialog in state/nlu.rs::interpret — must report Fulfilled for an intent the bot completed itself (no fulfillment code hook, closing response played), reserving ReadyForFulfillment for fulfilment the bot does not perform.
Traces: lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent, lexv2-runtime.recognize-text-rejects-an-unbuilt-locale (both fail exactly here; lexv2-runtime.session-state-persists-across-turns asserts the same on turn two).

Cause 2 of 5 — Intent.slots is omitted entirely while no slot has a value, instead of naming the intent's slots with a null value.
AWS: Smithy lexruntimev2#Intent.slots is a Slots map documented as "A map of all of the slots for the intent. The name of the slot maps to the value of the slot. If a slot has not been filled, the value is null." So a turn that elicits a Required slot reports slots as a PRESENT map containing that slot's name mapped to null — {"<slotName>": null} for an intent with one slot — and GetSession on the same four labels reports the same map.
Emulator: the member is absent, so reading it raises "KeyError: 'slots'" on the elicitation turn (dialogAction ElicitSlot/slotToElicit and intent.state InProgress are now correct, only slots is missing). state/nlu.rs builds the Intent with `slots: (!slots.is_empty()).then_some(slots)`, so an intent with nothing filled yet serializes no slots member at all.
Why: every Lex dialog loop indexes sessionState.intent.slots to see which slots are still open; a missing key faults client code written against AWS, and a client cannot tell "slot declared and unfilled" from "intent has no slots".
Fix: the turn component must always populate Intent.slots, keyed by every slot the recognized intent declares, with a null value for each slot that has none, and persist that map on the session so GetSession reports it too.
Traces: lexv2-runtime.recognize-text-elicits-missing-required-slot.

Cause 3 of 5 — SessionState.originatingRequestId is never minted; it comes back only when the caller supplied one.
AWS: Smithy lexruntimev2#SessionState.originatingRequestId is a NonEmptyString (len 1..) documented as "A unique identifier for a specific request". The service mints it and reports it in the sessionState of every turn and of GetSession; all turns of one conversation report the SAME value, so a client can correlate them.
Emulator: the member is absent from the opening RecognizeText turn — "KeyError: 'originatingRequestId'" reading first['sessionState']['originatingRequestId'] — even though that turn correctly left dialogAction ElicitSlot. state.rs::merge_session_state (around line 616) sets session_state.originating_request_id only from a caller-supplied value, and nothing in the crate ever generates one.
Why: originatingRequestId is how a client ties the turns of one conversation together and how it tells a resumed conversation from a fresh one; absent, that correlation is impossible and code reading the member faults.
Fix: the runtime's session state must mint an originatingRequestId when a session comes into existence and report it on every sessionState it serializes (RecognizeText, RecognizeUtterance, PutSession, GetSession), keeping it stable for the life of the session.
Traces: lexv2-runtime.session-state-persists-across-turns.

Cause 4 of 5 — an unknown BotId/BotAliasId pair is refused with the BotId/BotLocale message; the two refusals are collapsed into one.
AWS: Smithy lexruntimev2#RecognizeText models ResourceNotFoundException (httpError 404). AWS distinguishes two refusals that the emulator now answers identically. An alias id that is well formed but is not an alias of that bot is refused with HTTP 404, Error.Code ResourceNotFoundException and Error.Message "<botId>/<botAliasId> is not a valid BotId/BotAliasId combination." — the fixed text " is not a valid BotId/BotAliasId combination." after the botId and botAliasId taken verbatim from the request path. The separate case of a locale that is not built keeps the entirely fixed sentence "The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale combination and retry your request."
Emulator: the code is now right but the message is the wrong one — "AssertionError: got message='The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale combination and retry your request.'" for a request whose botAliasId is not an alias of the bot. state/bot_model.rs collapses both conditions into LocaleUnavailable::NotBuilt (the alias lookup at ~line 234-238 and the status check at ~line 246 both return it) and recognize_text.rs maps that one variant to BOT_LOCALE_NOT_VALID_MESSAGE.
Why: the message is the only thing that tells the two misconfigurations apart. A client whose deploy points at a deleted or typo'd alias is told its locale is unbuilt and goes off building a locale that is already Built; log/alert matching on either sentence misclassifies the other.
Fix: the locale-resolution component must distinguish "the BotId/BotAliasId pair does not resolve" from "the resolved locale is not Built" and RecognizeText/RecognizeUtterance must render the ResourceNotFoundException message for the condition actually hit, with the botId and botAliasId from the request filling the first sentence's variable parts.
Traces: lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias.

Cause 5 of 5 — RecognizeUtterance with a text responseContentType returns the message text as the audioStream payload instead of an empty body.
AWS: Smithy lexruntimev2#RecognizeUtteranceResponse.audioStream is the @httpPayload BlobStream, default "", documented as "The prompt or statement to send to the user" — i.e. SYNTHESIZED SPEECH. RecognizeUtteranceRequest.responseContentType selects text or speech; with the text value ("text/plain; charset=utf-8") the bot's messages are returned in the gzip+base64 x-amz-lex-messages header and no speech is synthesized, so the response body is empty (0 bytes) and Content-Type echoes the requested text type.
Emulator: the body carries the message text — "AssertionError: a text response still carried 25 bytes of audio (b'Your shake is on the way.'); asking for text must suppress speech synthesis". ops/recognize_utterance.rs (the audio_stream binding near the end of handle) deliberately joins the turn's message contents with newlines and returns those bytes as audioStream for a text response.
Why: audioStream is an audio payload. A client that plays or writes the body gets the message text where AWS gives it nothing, it double-renders the message it already decoded from x-amz-lex-messages, and any "did the bot speak?" check on body length is inverted.
Fix: RecognizeUtterance's handler/serializer must leave audioStream empty whenever responseContentType selects text (the comparison must tolerate the whitespace variants of the media type, e.g. "text/plain; charset=utf-8" as the model writes it), and emit a payload only for a speech response content type.
Traces: lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text.

## Evidence
Suite: lexv2-runtime 0/10 (0%), 10 failed; lexv2-models__lexv2-runtime 4/4 (100%). Four of the ten failures are marked OWNED by existing open issues and are skipped. The six clustered here:

1) [mismatch] lexv2-runtime.recognize-text-fills-slot-and-fulfills-intent
   AssertionError: intent state was 'ReadyForFulfillment', expected 'Fulfilled'
   Call: RecognizeText(botId, botAliasId, localeId=en_US, sessionId=<fresh>, text="i would like a chocolate shake").
   Expected (AWS): dialogAction.type Close (now correct), intent.name OrderShake (now correct), intent.state "Fulfilled".
   Actual: intent.state "ReadyForFulfillment".

2) [mismatch] lexv2-runtime.recognize-text-rejects-an-unbuilt-locale
   AssertionError: after the build the intent state was 'ReadyForFulfillment', expected 'Fulfilled'
   The NotBuilt refusal now matches; the positive control after BuildBotLocale reports the wrong closed-turn state. Same cause as (1).

3) [error] lexv2-runtime.recognize-text-elicits-missing-required-slot
   KeyError: 'slots' at assert intent["slots"] == {"Flavor": None}
   Call: RecognizeText(... text="order a shake"). dialogAction.type ElicitSlot, dialogAction.slotToElicit "Flavor" and intent.state "InProgress" are all now correct.
   Expected (AWS): intent.slots present as {"Flavor": null}; GetSession reports the same map.
   Actual: the Intent carries no slots member at all.

4) [error] lexv2-runtime.session-state-persists-across-turns
   KeyError: 'originatingRequestId' at origin = first["sessionState"]["originatingRequestId"]
   Call: RecognizeText(... text="order a shake", sessionState={"sessionAttributes": {"orderChannel": "kiosk"}}). Turn one's dialogAction ElicitSlot assertion now passes.
   Expected (AWS): sessionState.originatingRequestId present on turn one and identical on turn two.
   Actual: the member is absent from the response.

5) [mismatch] lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias
   AssertionError: got message='The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale combination and retry your request.'
   Call: RecognizeText(botId=<bot>, botAliasId="XXXXXXXXXX", localeId=en_US, sessionId=<fresh>, text="order a shake"). Error.Code "ResourceNotFoundException" is now correct.
   Expected (AWS): Error.Message "<botId>/XXXXXXXXXX is not a valid BotId/BotAliasId combination." — fixed text after the botId and botAliasId from the request path.
   Actual: the unbuilt-locale sentence, which carries no variable part at all.

6) [mismatch] lexv2-runtime.recognize-utterance-text-stream-agrees-with-recognize-text
   AssertionError: a text response still carried 25 bytes of audio (b'Your shake is on the way.'); asking for text must suppress speech synthesis
   Call: RecognizeUtterance(... requestContentType="text/plain; charset=utf-8", responseContentType="text/plain; charset=utf-8", inputStream=b"i would like a strawberry shake"). sessionId, inputMode "Text" and contentType all now correct.
   Expected (AWS): the response body (audioStream) is empty; the bot's closing message is carried only by x-amz-lex-messages.
   Actual: the body holds the message text.

Source evidence (service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0): src/state/nlu.rs:76 and :188-194 (INTENT_STATE_READY_FOR_FULFILLMENT on the closing branch) and nlu.rs:28-31 (the rule stated in the module doc); src/state/nlu.rs:205 (`slots: (!slots.is_empty()).then_some(slots)`); src/state.rs:609-630 merge_session_state, which only echoes a caller-supplied originating_request_id, and src/state.rs:509-521 Session::new, which mints none; src/state/bot_model.rs:36-38 BOT_LOCALE_NOT_VALID_MESSAGE with :234-248 returning LocaleUnavailable::NotBuilt for an unresolved alias AND an unbuilt locale, consumed by src/ops/recognize_text.rs:252-258; src/ops/recognize_utterance.rs audio_stream (the join of turn.messages into the httpPayload) around lines 583-610.

## Steps to Reproduce
1. lexv2-models: CreateBot with an IAM role lexv2.amazonaws.com can assume; wait for botStatus Available. CreateBotLocale botVersion DRAFT, localeId en_US, nluIntentConfidenceThreshold 0.40. CreateSlotType "Flavor" with sample values chocolate/vanilla/strawberry. CreateIntent "OrderShake" with sampleUtterances ["i would like a {Flavor} shake", "order a shake", "i would like a shake"] and intentClosingSetting closingResponse "Your shake is on the way.". CreateSlot "Flavor" Required with promptSpecification "What flavor would you like?"; UpdateIntent with slotPriorities. Note the bot's DRAFT test alias id.
2. Cause 4: BEFORE BuildBotLocale is irrelevant here — after the bot exists, call lexv2-runtime RecognizeText(botId, botAliasId="XXXXXXXXXX", localeId=en_US, sessionId=<fresh>, text="order a shake"). Emulator: 404 ResourceNotFoundException with the BotId/BotLocale sentence. AWS: 404 ResourceNotFoundException "<botId>/XXXXXXXXXX is not a valid BotId/BotAliasId combination."
3. BuildBotLocale(botId, DRAFT, en_US) and poll DescribeBotLocale until botLocaleStatus Built.
4. Cause 1: RecognizeText(botId, <real alias>, en_US, <fresh session>, text="i would like a chocolate shake") and read sessionState.intent["state"]. Emulator: "ReadyForFulfillment". AWS: "Fulfilled".
5. Cause 2: in a fresh session, RecognizeText(... text="order a shake") and read sessionState.intent["slots"]. Emulator: KeyError, the member is absent. AWS: {"Flavor": null}. Then GetSession on the same four labels and read the same member — AWS reports the same map.
6. Cause 3: on that same opening turn, read sessionState["originatingRequestId"] — absent on the emulator; present on AWS and identical on the next turn of the session.
7. Cause 5: RecognizeUtterance(botId, <real alias>, en_US, <fresh session>, requestContentType="text/plain; charset=utf-8", responseContentType="text/plain; charset=utf-8", inputStream=b"i would like a strawberry shake") and read the response body. Emulator: 25 bytes, b"Your shake is on the way.". AWS: 0 bytes, with that message only in the gzip+base64 x-amz-lex-messages header.
