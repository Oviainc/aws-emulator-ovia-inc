Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Lex Runtime V2 emulator

_Filed: 2026-10-21T21:08:50Z_
_Operation: service-com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`), so no session can be created, read or deleted. Implement a FIRST, SMALL stateful slice: the session lifecycle of the service's primary resource (a bot-alias-locale session), wired end-to-end over the `restJson1` protocol (sigv4 signing name `lex`, endpoint prefix `runtime-v2-lex`).

### Happy path to implement (in order)

1. **PutSession** — `POST /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}`. Inputs: `botId` (len 10..10, `^[0-9a-zA-Z]+$`), `botAliasId`, `localeId`, `sessionId` (len 2..100, `^[0-9a-zA-Z._:-]+$`) — all `@httpLabel` and required — plus body members `sessionState` (required; `dialogAction`, `intent`, `activeContexts`, `sessionAttributes`), `messages` (0..10 `Message{content, contentType, imageResponseCard}`), `requestAttributes`, and header `ResponseContentType`. Must CREATE or REPLACE the stored session under the 4-part key and persist the supplied `sessionState`, `messages` and `requestAttributes`. The output binds everything to HEADERS: `x-amz-lex-session-id`, `x-amz-lex-session-state`, `x-amz-lex-messages`, `x-amz-lex-request-attributes` (each the JSON-serialized stored value, not the raw request echo), `Content-Type`, and the required `@httpPayload` `audioStream` blob (empty blob is acceptable for a text session).
2. **GetSession** — `GET /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}`. Must READ from the store and return `sessionId`, `sessionState`, `messages` (0..10) and `interpretations` (0..5) exactly as last persisted by PutSession/RecognizeText. If no session exists for the key, return `ResourceNotFoundException` (HTTP 404) — never an empty 200.
3. **RecognizeText** — `POST .../sessions/{sessionId}/text`. Inputs: the 4 labels plus required `text` (len 1..1024), optional `sessionState` and `requestAttributes`. Must load the existing session (creating one implicitly when absent, as the real service does), merge any supplied `sessionState`/`requestAttributes` into it, advance and PERSIST the session state, and return `messages`, `sessionState`, `interpretations` (each `Interpretation{intent, nluConfidence, interpretationSource}`), `requestAttributes`, `sessionId` and optionally `recognizedBotMember`. The returned `sessionState.sessionAttributes` and `intent` must be the stored ones, so a following GetSession observes the same values. A deterministic rule-based interpretation (e.g. echo intent `FallbackIntent` with `dialogAction.type = Close` unless the caller supplied an `intent`) is fine — but it must be computed from and written back to state, not a constant that is discarded.
4. **DeleteSession** — `DELETE /bots/{botId}/botAliases/{botAliasId}/botLocales/{localeId}/sessions/{sessionId}`. Must REMOVE the stored session and echo back `botId`, `botAliasId`, `localeId`, `sessionId`. A subsequent GetSession for that key must then fail with `ResourceNotFoundException` (404). Deleting a non-existent session must also return 404.

All four must validate the `@httpLabel` constraints (`botId` exactly 10 alphanumerics; `sessionId` 2..100 matching `^[0-9a-zA-Z._:-]+$`; `localeId` non-empty) and reject violations with `ValidationException` (HTTP 400). PutSession must reject a missing `sessionState` with `ValidationException`.

NO canned or echoed responses: each of the four must compute its response from the persisted session, and a create followed by a read must return what was created. A hollow 200, a hardcoded body, or a generated session id that is never stored is a failure of this issue.

## State Model

In `ComAmazonawsLexruntimev2Awsdeepsenseruntimeserviceapi20State`:

- `sessions: HashMap<SessionKey, Session>`
- `SessionKey { bot_id: String, bot_alias_id: String, locale_id: String, session_id: String }` — the 4 `@httpLabel` values; two sessions with the same `sessionId` under different bots/aliases/locales are distinct.
- `Session { session_id: String, session_state: SessionState, messages: Vec<Message>, interpretations: Vec<Interpretation>, request_attributes: HashMap<String, String>, created_at: SystemTime, updated_at: SystemTime, turns: u64 }` where `session_state` carries the modeled `SessionState` members (`dialogAction{type, slotToElicit, ...}`, `intent{name, slots, state, confirmationState}`, `activeContexts` (0..20), `sessionAttributes` map, `originatingRequestId`, `runtimeHints`).
- `turns`/`updated_at` are bumped by RecognizeText so repeated calls are observably stateful.

## Out of scope for this slice

`RecognizeUtterance` (audio/blob payload with header-bound JSON) and `StartConversation` (declined by the stub generator — `@streaming` event-stream input, see `src/ops/SKIPPED_STUBS.md`) stay unimplemented/501 and will be filed as individual gap issues later.

Done = a client script running PutSession → GetSession → RecognizeText → GetSession → DeleteSession → GetSession(404) passes with correct, stateful bodies/headers, not just 2xx.

## Evidence

OPERATIONS.md (crate root) lists 6 operations for `restJson1` / SDK id `Lex Runtime V2`, with the routes, labels, required members, output members and error sets quoted above (DeleteSession lines 26-31, GetSession 33-38, PutSession 40-45, RecognizeText 47-52, RecognizeUtterance 54-59, StartConversation 61-66). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 5 generated handlers (delete_session.rs, get_session.rs, put_session.rs, recognize_text.rs, recognize_utterance.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501; src/ops/SKIPPED_STUBS.md records StartConversation as modeled-but-unwired. src/state.rs defines the per-service state struct with only `// TODO: add service-specific state fields`, so nothing can persist today. Shape members cited in the State Model come from smithy_query on `SessionState`, `Message`, `Interpretation`, `Intent` and `DialogAction`. Session-lifecycle semantics (GetSession/DeleteSession on an unknown session -> ResourceNotFoundException 404; RecognizeText implicitly starting a session and carrying sessionAttributes forward) are the documented Lex Runtime V2 behaviors and the errors are declared on each operation in the model.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `lexv2-runtime` client at it.
2. PutSession(botId='ABCDEFGHIJ', botAliasId='TSTALIASID', localeId='en_US', sessionId='sess-001', sessionState={sessionAttributes:{'cart':'2'}, dialogAction:{type:'Delegate'}}) — expect 200 with x-amz-lex-session-id: sess-001 and x-amz-lex-session-state containing the sessionAttributes. Today: HTTP 501 NotImplemented.
3. GetSession(same 4 labels) — expect 200 whose sessionState.sessionAttributes == {'cart':'2'}. Today: 501.
4. RecognizeText(same labels, text='I want a pizza') — expect 200 with non-empty messages/interpretations and a sessionState persisted for the session. Today: 501.
5. GetSession again — expect the sessionState/interpretations written by RecognizeText (proves state is shared across handlers, not per-request).
6. DeleteSession(same labels) — expect 200 echoing botId/botAliasId/localeId/sessionId. Today: 501.
7. GetSession once more — expect ResourceNotFoundException with HTTP 404.
8. Negative: GetSession with botId='SHORT' (not 10 chars) or sessionId='x' (len 1) — expect ValidationException with HTTP 400.
