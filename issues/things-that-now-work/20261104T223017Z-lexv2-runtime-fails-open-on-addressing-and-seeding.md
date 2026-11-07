Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias lexv2-runtime.recognize-text-rejects-an-unbuilt-locale lexv2-runtime.put-session-rejects-an-intent-the-locale-does-not-define lexv2-runtime.put-session-seeds-dialog-state-for-the-next-turn
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias|_client_error#6|AssertionError|raise AssertionError("the call unexpectedly succeeded; expected a ClientError")
Issue-Key: lexv2-runtime.recognize-text-rejects-an-unbuilt-locale|_client_error#7|AssertionError|raise AssertionError("the call unexpectedly succeeded; expected a ClientError")
Issue-Key: lexv2-runtime.put-session-rejects-an-intent-the-locale-does-not-define|_client_error#4|AssertionError|raise AssertionError("the call unexpectedly succeeded; expected a ClientError")
Issue-Key: lexv2-runtime.put-session-seeds-dialog-state-for-the-next-turn|run#8|KeyError|assert [m["content"] for m in _decode(put["messages"])] == [_PROMPT], (
Issue-Commit: 3e82efa9dc9c0e3c4a597a967c25a995e0e693a6

# lexv2-runtime fails open on addressing and seeding: RecognizeText accepts an unknown botAliasId and a NotBuilt locale with 200, PutSession accepts an intent the locale does not define, and PutSession omits the elicited slot's prompt from x-amz-lex-messages (4 causes)

> _Note: this issue was identified against the code base at commit `3e82efa9dc9c0e3c4a597a967c25a995e0e693a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RecognizeText

## Description
The Lex V2 runtime keys its session store on the four @httpLabel values alone (services/service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0/src/state.rs SessionKey::new) and never resolves the bot, alias, locale or intent those labels name, so no precondition of a turn or a seed is checked: both RecognizeText and PutSession accept input real AWS refuses, create a session for it, and PutSession returns none of the bot's authored messages. Four distinct divergences, all of which must be closed before this issue is submitted.

Related open issue: 20261104T180144Z-lexv2-models-lexv2-runtime-lexv2-runtime-recognizetext-has-no-nlu-or-dialog.md covers the unbuilt-locale refusal from the lexv2-models__lexv2-runtime check only; it does NOT carry any of the lexv2-runtime check ids below, and causes 1, 3 and 4 here (the alias pair, PutSession's intent validation, PutSession's messages header) appear in no open issue.

Cause 1 of 4 — a well-formed botAliasId that is not this bot's is accepted with 200 and opens a session.
AWS: Smithy lexruntimev2#RecognizeText models ResourceNotFoundException (httpError 404). The runtime resolves the BotId/BotAliasId PAIR; when the alias is not an alias of that bot the call is refused with HTTP 404, Error.Code ResourceNotFoundException and Error.Message "<botId>/<botAliasId> is not a valid BotId/BotAliasId combination." — fixed text " is not a valid BotId/BotAliasId combination." after the botId and botAliasId taken verbatim from the request path. The refusal has no effect: no session is created, so a following GetSession for that sessionId on the bot's real alias is itself 404 ResourceNotFoundException with "The session for user <sessionId> doesn\u2019t exist or has expired." (fixed text around the sessionId from the request; apostrophe U+2019).
Emulator: the call succeeds — "AssertionError: the call unexpectedly succeeded; expected a ClientError" for botAliasId "XXXXXXXXXX" on a bot whose alias is different — and because advance_turn inserts the session on a first turn, a session now exists for labels AWS never served.
Why: this fails open on addressing. A client with a typo'd or stale alias id (a deploy pointing at a deleted alias) gets successful turns against the emulator and 404s in production, and the stray session makes later reads answer 200 where AWS answers 404.
Fix: RecognizeText's handler needs a precondition that resolves botId + botAliasId against the lexv2-models bot/alias state and returns the modeled ResourceNotFoundException with AWS's wording, ordered ahead of the session insert so a refused turn leaves no session.
Traces: lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias. Handler: src/ops/recognize_text.rs:209-252; key construction at src/ops/recognize_text.rs:223-228.

Cause 2 of 4 — RecognizeText against a locale that was never built is accepted with 200 and opens a session.
AWS: a bot locale whose botLocaleStatus is NotBuilt has no runtime model, so RecognizeText is refused with HTTP 404, Error.Code ResourceNotFoundException and the message (entirely fixed text, no variable parts) "The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale combination and retry your request." No session comes into existence: GetSession for that sessionId is 404 ResourceNotFoundException "The session for user <sessionId> doesn\u2019t exist or has expired.", and it stays 404 even after the locale is later built.
Emulator: the call succeeds — "AssertionError: the call unexpectedly succeeded; expected a ClientError" against a locale DescribeBotLocale reports as NotBuilt — and the accepted turn inserts the session, so the absence-of-effect property is lost too.
Why: a client testing its build gate, or its handling of "bot not built yet", sees a healthy turn where AWS refuses; a deployment that forgot BuildBotLocale looks fine against the emulator and 404s in production.
Fix: the same precondition in RecognizeText's handler must read the addressed locale's build state from lexv2-models state and refuse with the modeled ResourceNotFoundException unless it is Built, before any session is created or written.
Traces: lexv2-runtime.recognize-text-rejects-an-unbuilt-locale.

Cause 3 of 4 — PutSession accepts a sessionState.intent naming an intent the built locale does not define, and creates the session.
AWS: Smithy lexruntimev2#PutSession takes a required sessionState used "to set the current intent, attributes, context, and dialog action", and ValidationException (httpError 400) is a common error of the service. Seeding an intent the locale does not define is refused with HTTP 400, Error.Code ValidationException and Error.Message "The intent is invalid." — entirely fixed text. The refusal stores nothing: GetSession for that sessionId is 404 ResourceNotFoundException "The session for user <sessionId> doesn\u2019t exist or has expired." The identical call naming an intent the locale DOES define is accepted.
Emulator: the call succeeds — "AssertionError: the call unexpectedly succeeded; expected a ClientError" for sessionState {"dialogAction": {"type": "ElicitSlot", "slotToElicit": "Flavor"}, "intent": {"name": "NoSuchIntent", "state": "InProgress"}} — and src/ops/put_session.rs:288-309 writes the supplied state straight into the store, so a session seeded with a non-existent intent is readable afterwards.
Why: the seed is how applications resume or inject dialog state; accepting an undefined intent means a typo'd or renamed intent is only discovered in production, and every subsequent turn of that session is driven from a state AWS would never have stored.
Fix: PutSession's handler needs validation of the supplied sessionState against the addressed bot locale (intent name, and the slot named by dialogAction.slotToElicit) returning the modeled ValidationException with AWS's wording, ordered ahead of the store write so a refused seed creates no session.
Traces: lexv2-runtime.put-session-rejects-an-intent-the-locale-does-not-define. Handler: src/ops/put_session.rs:233-379.

Cause 4 of 4 — PutSession returns no x-amz-lex-messages for a seeded elicitation; the header is emitted only when the caller itself supplied messages.
AWS: Smithy lexruntimev2#PutSessionResponse.messages is the httpHeader x-amz-lex-messages, "a list of messages that were last sent to the user ... ordered based on how you return the messages from you Lambda function or the order that the messages are defined in the bot", gzip-compressed then base64 encoded. Seeding dialogAction {type ElicitSlot, slotToElicit <slot>} makes the bot enter that elicitation, so the response re-issues the slot's own valueElicitationSetting.promptSpecification message from the bot definition even though the request carried no messages of its own.
Emulator: the header is absent, so reading it raises "KeyError: 'messages'". src/ops/put_session.rs:294 stores session.messages = input.messages.unwrap_or_default() and :324-341 emits the header only when that stored list is non-empty — nothing is ever derived from the bot's authored prompts.
Why: a client seeding an elicitation reads the prompt to play back to the user out of this header; absent, there is nothing to say to the user, and code that decodes the header faults instead of getting the bot's own prompt.
Fix: PutSession must compute the messages of the dialog state it enters from the resolved bot locale (the elicited slot's promptSpecification for an ElicitSlot seed) when the request supplies none, and serialize them into the x-amz-lex-messages header with the existing gzip+base64 encoding.
Traces: lexv2-runtime.put-session-seeds-dialog-state-for-the-next-turn. Handler: src/ops/put_session.rs:294 and :324-341.

## Evidence
Suite: lexv2-runtime 0/10 (0%), 10 failed. The four checks clustered here:

1) [mismatch] lexv2-runtime.recognize-text-rejects-an-unknown-bot-alias
   AssertionError: the call unexpectedly succeeded; expected a ClientError
   Call: RecognizeText(botId=<bot>, botAliasId="XXXXXXXXXX", localeId=en_US, sessionId=<fresh>, text="order a shake").
   Expected (AWS): HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "<botId>/<botAliasId> is not a valid BotId/BotAliasId combination." (fixed text after the two ids from the request); then GetSession on the bot's REAL alias for that sessionId is 404 ResourceNotFoundException "The session for user <sessionId> doesn\u2019t exist or has expired."
   Actual (emulator): HTTP 200 with a turn, and the session exists.

2) [mismatch] lexv2-runtime.recognize-text-rejects-an-unbuilt-locale
   AssertionError: the call unexpectedly succeeded; expected a ClientError
   Expected (AWS) while DescribeBotLocale reports botLocaleStatus "NotBuilt": HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale combination and retry your request." (entirely fixed text); GetSession for that sessionId is 404 ResourceNotFoundException "The session for user <sessionId> doesn\u2019t exist or has expired.", still 404 after BuildBotLocale.
   Actual (emulator): HTTP 200, and the refused-in-AWS turn becomes a session.

3) [mismatch] lexv2-runtime.put-session-rejects-an-intent-the-locale-does-not-define
   AssertionError: the call unexpectedly succeeded; expected a ClientError
   Call: PutSession(... sessionState={"dialogAction": {"type": "ElicitSlot", "slotToElicit": "Flavor"}, "intent": {"name": "NoSuchIntent", "state": "InProgress"}}).
   Expected (AWS): HTTP 400, Error.Code "ValidationException", Error.Message "The intent is invalid." (entirely fixed text); then GetSession for that sessionId is 404 ResourceNotFoundException "The session for user <sessionId> doesn\u2019t exist or has expired." — the rejected call must not have created a session.
   Actual (emulator): HTTP 200 and the session is stored.

4) [error] lexv2-runtime.put-session-seeds-dialog-state-for-the-next-turn
   KeyError: 'messages' at [m["content"] for m in _decode(put["messages"])]
   Call: PutSession(... sessionState={"dialogAction": {"type": "ElicitSlot", "slotToElicit": "Flavor"}, "intent": {"name": "OrderShake", "state": "InProgress"}, "sessionAttributes": {"seededBy": "put-session"}}) with no messages in the request.
   Expected (AWS): the x-amz-lex-messages header is present and decodes (base64 then gzip) to one message whose content is the slot's authored prompt "What flavor would you like?".
   Actual (emulator): no x-amz-lex-messages header at all.

Source evidence: ops/recognize_text.rs:223-239 builds SessionKey from the four labels and advances the turn with no resolution of bot/alias/locale; ops/put_session.rs:266-309 checks only attribute byte budgets and the reserved-namespace rule before writing the supplied sessionState into the store; ops/put_session.rs:294 and :324-341 emit x-amz-lex-messages only from input.messages.

## Steps to Reproduce
1. lexv2-models: CreateBot / CreateBotLocale (DRAFT, en_US, nluIntentConfidenceThreshold 0.40) / CreateSlotType "Flavor" (chocolate, vanilla, strawberry) / CreateIntent "OrderShake" with a closingResponse / CreateSlot "Flavor" Required with promptSpecification "What flavor would you like?"; note the bot's real alias id.
2. Cause 2: BEFORE BuildBotLocale, with DescribeBotLocale reporting NotBuilt, call lexv2-runtime RecognizeText(botId, botAliasId=<the bot's alias>, localeId=en_US, sessionId=<fresh>, text="i would like a chocolate shake"). Emulator returns 200; AWS returns 404 ResourceNotFoundException "The provided BotId/BotLocale combination is not valid. Please update the BotId/BotLocale combination and retry your request." Then GetSession for that sessionId: AWS 404, emulator finds the session the accepted call created — still found after step 3.
3. BuildBotLocale and wait for botLocaleStatus Built.
4. Cause 1: RecognizeText(botId, botAliasId="XXXXXXXXXX", localeId=en_US, sessionId=<fresh>, text="order a shake"). Emulator 200; AWS 404 ResourceNotFoundException "<botId>/XXXXXXXXXX is not a valid BotId/BotAliasId combination." Then GetSession on the bot's real alias for that sessionId: AWS 404 "The session for user <sessionId> doesn\u2019t exist or has expired."
5. Cause 3: PutSession(botId, botAliasId=<real alias>, localeId=en_US, sessionId=<fresh>, sessionState={"dialogAction":{"type":"ElicitSlot","slotToElicit":"Flavor"},"intent":{"name":"NoSuchIntent","state":"InProgress"}}). Emulator 200; AWS 400 ValidationException "The intent is invalid.", and GetSession for that sessionId is then 404. The same call with intent name "OrderShake" must still be accepted.
6. Cause 4: PutSession with the same dialogAction but intent name "OrderShake" and NO messages in the request, then base64-decode and gunzip the x-amz-lex-messages response header. Emulator: the header is missing (KeyError). AWS: it decodes to [{"content": "What flavor would you like?", ...}].
