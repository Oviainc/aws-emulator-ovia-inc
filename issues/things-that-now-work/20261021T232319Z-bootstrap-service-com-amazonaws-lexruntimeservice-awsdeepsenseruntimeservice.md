Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 96234e472a4dbed875261b6f3ae0a9ed15fa6e7c

# Bootstrap the Lex Runtime Service emulator

_Filed: 2026-10-21T23:23:19Z_
_Operation: service-com-amazonaws-lexruntimeservice-awsdeepsenseruntimeservice | Class: missing_

> _Note: this issue was identified against the code base at commit `96234e472a4dbed875261b6f3ae0a9ed15fa6e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations in this crate (`src/ops/*.rs`) is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns HTTP 501 `NotImplemented`, holding no state. A client cannot complete a single conversation turn.

Implement a FIRST stateful slice covering the core Lex runtime conversation loop — a per-(botName, botAlias, userId) session that is created, advanced by a text turn, read back, and deleted. Scope is exactly these 4 operations, in this order:

1. **PutSession** — `POST /bot/{botName}/alias/{botAlias}/user/{userId}/session`. Input: `botName`, `botAlias`, `userId` (@httpLabel, required; `userId` len 2..100, pattern `^[0-9a-zA-Z._:-]+$`), `sessionAttributes` (map<string,string>), `dialogAction` (DialogAction: required `type`, plus `intentName`, `slots`, `slotToElicit`, `fulfillmentState`, `message`, `messageFormat`), `recentIntentSummaryView` (list<IntentSummary>, len 0..3), `activeContexts` (list<ActiveContext>, len 0..20), `accept` (Accept header). Must CREATE or REPLACE the stored session and return, as @httpHeader values, the state actually persisted: `x-amz-lex-session-id` (a generated, stable session id), `x-amz-lex-intent-name`, `x-amz-lex-slots`, `x-amz-lex-session-attributes`, `x-amz-lex-dialog-state`, `x-amz-lex-slot-to-elicit`, `x-amz-lex-message`/`x-amz-lex-message-format` when the input `dialogAction` carried a message, `x-amz-lex-active-contexts`, plus the required `audioStream` @httpPayload (empty blob is acceptable for `text/plain`-style accepts). `dialogState` must be derived from `dialogAction.type`/`fulfillmentState` (e.g. `ElicitSlot` -> `ElicitSlot`, `Close`+`Fulfilled` -> `Fulfilled`, `ConfirmIntent` -> `ConfirmIntent`), not hardcoded.

2. **PostText** — `POST /bot/{botName}/alias/{botAlias}/user/{userId}/text`. Input: labels as above, `inputText` (len 1..1024, required), `sessionAttributes`, `requestAttributes`, `activeContexts`. Must LOAD the session created by PutSession (creating one on first contact if absent), MERGE `sessionAttributes` into the stored map, record the turn, and return a JSON body computed from that stored session: `sessionId` (same id PutSession returned), `intentName`, `slots`, `sessionAttributes` (the merged map), `dialogState`, `slotToElicit`, `message`, `messageFormat`, `botVersion`, `activeContexts`. The merged `sessionAttributes` and the carried-forward `intentName`/`slots` MUST be observable on the next call — not echoed from the request only.

3. **GetSession** — `GET /bot/{botName}/alias/{botAlias}/user/{userId}/session`. Must read the SAME stored session and return `sessionId`, `sessionAttributes`, `dialogAction` (reconstructed from stored dialog state), `recentIntentSummaryView` (truncated to len 0..3, most recent first) and `activeContexts` (len 0..20). When `checkpointLabelFilter` (@httpQuery, len 1..255, pattern `^[a-zA-Z0-9-]+$`) is present, filter `recentIntentSummaryView` to summaries whose `checkpointLabel` equals it. An unknown bot/user must return `NotFoundException` (HTTP 404), not an empty 200.

4. **DeleteSession** — `DELETE /bot/{botName}/alias/{botAlias}/user/{userId}/session`. Must REMOVE the stored session and echo back `botName`, `botAlias`, `userId` and the `sessionId` that was deleted. A subsequent GetSession for the same key must not return the deleted conversation state (fresh/empty session or `NotFoundException`).

Rules for this slice:
- No canned, echoed-only, or hardcoded responses, and no random id that is not stored: every field above must be computed from persisted state, and the `sessionId` returned by PutSession must be the id returned by the following PostText and GetSession.
- Enforce the modeled constraints that are cheap and observable: `userId` length/pattern, `inputText` length 1..1024, `recentIntentSummaryView` max 3, `activeContexts` max 20, `checkpointLabelFilter` pattern — reject with `BadRequestException` (HTTP 400); missing session with `NotFoundException` (HTTP 404).
- PostContent stays stubbed (501) for now and will be filed as its own gap issue; it should later share the same session store.

## State Model

In `src/state.rs`, on the shared per-service state:

- `sessions: HashMap<SessionKey, LexSession>` where `SessionKey { bot_name: String, bot_alias: String, user_id: String }`.
- `LexSession {
    session_id: String,            // generated once at session creation, stable for the session's life
    bot_name: String,
    bot_alias: String,
    user_id: String,
    bot_version: String,           // e.g. "$LATEST" (model pattern ^[0-9]+|\$LATEST$)
    session_attributes: HashMap<String, String>,
    request_attributes: HashMap<String, String>,   // per-turn, not persisted across turns
    dialog: DialogStateRecord,
    recent_intents: Vec<IntentSummaryRecord>,      // newest first, capped at 3
    active_contexts: Vec<ActiveContextRecord>,     // capped at 20
    turns: Vec<Turn>,
    created_at: SystemTime,
    last_updated_at: SystemTime,
  }`
- `DialogStateRecord { dialog_state: DialogState, intent_name: Option<String>, slots: HashMap<String, String>, slot_to_elicit: Option<String>, fulfillment_state: Option<FulfillmentState>, message: Option<String>, message_format: Option<MessageFormatType> }`
- `IntentSummaryRecord { intent_name: Option<String>, checkpoint_label: Option<String>, slots: HashMap<String, String>, confirmation_status: Option<ConfirmationStatus>, dialog_action_type: DialogActionType, fulfillment_state: Option<FulfillmentState>, slot_to_elicit: Option<String> }`
- `ActiveContextRecord { name: String, time_to_live: ActiveContextTimeToLive, parameters: HashMap<String, String> }`
- `Turn { input_text: String, intent_name: Option<String>, dialog_state: DialogState, at: SystemTime }`

All handlers reach this store via `crate::state::service_state(&*self.engine)` and the lock guard, so a write by PutSession/PostText is visible to GetSession and removable by DeleteSession.

Done = an integration test doing PutSession -> PostText -> GetSession -> DeleteSession -> GetSession passes with correct bodies/headers, not merely 2xx.

## Evidence

OPERATIONS.md for this crate lists all 5 operations with routes, input/output members and errors (DeleteSession, GetSession, PostContent, PostText, PutSession; restJson1, sigv4 name `lex`, endpoint prefix `runtime.lex`). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-lexruntimeservice-awsdeepsenseruntimeservice/src/ops` matches all five handler files (delete_session.rs, get_session.rs, post_content.rs, post_text.rs, put_session.rs), each returning `Scaffolded::not_implemented(..)` => HTTP 501 and persisting nothing. Field/shape details for DialogAction (required `type`), IntentSummary (required `dialogActionType`, `checkpointLabel` len 1..255 pattern ^[a-zA-Z0-9-]+$) and ActiveContext (required `name`, `timeToLive`, `parameters`) come from smithy_query on those shapes. Session continuity across PutSession/PostText/GetSession and session removal by DeleteSession are documented Amazon Lex V1 runtime semantics (session keyed by bot name + alias + userId).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `lex-runtime` client at it (endpoint prefix runtime.lex, signing name lex).
2. PutSession(botName='OrderFlowers', botAlias='prod', userId='user-1', sessionAttributes={'cart':'1'}, dialogAction={type:'ElicitSlot', intentName:'OrderFlowers', slotToElicit:'FlowerType', slots:{}}) — expect 200 with x-amz-lex-session-id set, x-amz-lex-dialog-state='ElicitSlot', x-amz-lex-intent-name='OrderFlowers'; today: HTTP 501 NotImplemented.
3. PostText(same bot/alias/userId, inputText='roses', sessionAttributes={'promo':'spring'}) — expect 200 whose sessionId equals the id from step 2 and whose sessionAttributes contains BOTH 'cart' and 'promo'; today: HTTP 501.
4. GetSession(same bot/alias/userId) — expect 200 returning the persisted sessionAttributes, a dialogAction reconstructed from the stored dialog state, and recentIntentSummaryView of at most 3 entries; today: HTTP 501.
5. GetSession(same key, checkpointLabelFilter='checkpoint-1') — expect recentIntentSummaryView filtered to summaries with that checkpointLabel.
6. DeleteSession(same key) — expect 200 echoing botName/botAlias/userId plus the deleted sessionId; today: HTTP 501.
7. GetSession(same key) again — the previous conversation state must be gone (NotFoundException/empty fresh session), proving the delete mutated persisted state.
8. GetSession(botName='Nope', botAlias='prod', userId='user-1') — expect NotFoundException (HTTP 404), not an empty 200.
