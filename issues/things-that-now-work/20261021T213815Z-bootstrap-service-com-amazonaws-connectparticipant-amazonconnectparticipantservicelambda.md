Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the connectparticipant emulator

_Filed: 2026-10-21T21:38:15Z_
_Operation: service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 11 operations of Amazon Connect Participant Service are unmodified scaffolds answering HTTP 501 `NotImplemented` with zero persisted state. Implement a FIRST stateful slice covering the core chat-participant happy path, in this order:

1. **CreateParticipantConnection** (`POST /participant/connection`) — input: `ParticipantToken` (header `X-Amz-Bearer`, required), `Type` (list of ConnectionType: WEBSOCKET | CONNECTION_CREDENTIALS), `ConnectParticipant` (bool). Must mint a NEW chat session keyed by the participant token and return `ConnectionCredentials{ConnectionToken, Expiry}` when `CONNECTION_CREDENTIALS` is requested and `Websocket{Url, ConnectionExpiry}` when `WEBSOCKET` is requested. The returned `ConnectionToken` must be a freshly generated opaque value that is STORED and is the key every later operation authenticates with; `Expiry`/`ConnectionExpiry` are ISO-8601 (`yyyy-MM-ddThh:mm:ss.SSSZ`) timestamps in the future. Omit the members whose type was not requested.

2. **SendMessage** (`POST /participant/message`) — input: `ContentType` (required, e.g. `text/plain`), `Content` (required, 1..16384), `ClientToken` (idempotency token), `ConnectionToken` (header, required). Must append an Item of type `MESSAGE` to the session's transcript and return the generated `Id` plus the `AbsoluteTime` it was stored with. Repeating the same `ClientToken` must return the SAME `Id`/`AbsoluteTime` and must not append a second transcript item.

3. **SendEvent** (`POST /participant/event`) — input: `ContentType` (required, e.g. `application/vnd.amazonaws.connect.event.typing`), `Content`, `ClientToken`, `ConnectionToken` (header). Must append an Item of type `EVENT` to the same transcript and return its stored `Id` and `AbsoluteTime`.

4. **GetTranscript** (`POST /participant/transcript`) — input: `ContactId`, `MaxResults` (0..100), `NextToken`, `ScanDirection` (BACKWARD|FORWARD), `SortOrder` (ASCENDING|DESCENDING), `StartPosition{Id|AbsoluteTime|MostRecent}`, `ConnectionToken` (header, required). Must return `Transcript` as the real list of previously sent items for the session resolved from the connection token (each `Item` carrying `Id`, `AbsoluteTime`, `Content`, `ContentType`, `Type`, `ParticipantId`, `DisplayName`, `ParticipantRole`), the session's `InitialContactId`, and a `NextToken` only when more items remain. Honour `SortOrder`/`ScanDirection` and slice by `MaxResults`; a returned `NextToken` fed back must continue exactly where the previous page stopped and must not repeat or drop items.

5. **DisconnectParticipant** (`POST /participant/disconnect`) — input: `ClientToken`, `ConnectionToken` (header, required). Must mark the session's connection as disconnected and return an empty 200. After it, subsequent SendMessage/SendEvent/GetTranscript calls with that same connection token must fail with `AccessDeniedException` (HTTP 403) rather than succeed.

Across all five: an unknown, malformed or already-disconnected `ConnectionToken` must produce `AccessDeniedException` (403), and inputs that violate the modeled length/range constraints (e.g. empty `Content`, `MaxResults` > 100, `ContentType` longer than 100 chars) must produce `ValidationException` (400) — not a 200 and not a 500.

Every one of these responses must be computed from persisted state. No canned bodies, no echoing the request back, no generating an `Id` or `ConnectionToken` that is never stored: a SendMessage followed by a GetTranscript must return exactly the message that was sent.

## State Model

In `src/state.rs` (reached via `crate::state::service_state(&*self.engine)`, locked through the shared engine):

- `sessions: HashMap<String /* ContactId */, ChatSession>`
- `connections: HashMap<String /* ConnectionToken */, String /* ContactId */>` — the token -> session index that authenticates every non-create operation
- `participant_tokens: HashMap<String /* ParticipantToken */, String /* ContactId */>` — so re-using a participant token resolves to the same session
- `ChatSession { contact_id: String, initial_contact_id: String, participant_id: String, participant_role: ParticipantRole, display_name: Option<String>, connection_token: String, connection_expiry: SystemTime, websocket_url: Option<String>, connected: bool, transcript: Vec<TranscriptItem>, idempotency: HashMap<String /* ClientToken */, String /* item Id */> }`
- `TranscriptItem { id: String, absolute_time: String /* ISO-8601 millis+Z */, content: Option<String>, content_type: String, item_type: ChatItemType /* MESSAGE | EVENT */, participant_id: String, display_name: Option<String>, participant_role: ParticipantRole, contact_id: String }`
- `transcript_pages: HashMap<String /* NextToken */, TranscriptCursor { contact_id: String, next_index: usize, sort_order: SortKey, scan_direction: ScanDirection }>` — so a NextToken resumes a real position instead of restarting

The remaining operations (StartAttachmentUpload, CompleteAttachmentUpload, GetAttachment, DescribeView, GetAuthenticationUrl, CancelParticipantAuthentication) stay stubbed at 501 for this slice and are tracked as individual gap issues later.

## Evidence

`services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/OPERATIONS.md` lists all 11 operations with their routes, input/output members and errors (restJson1, SDK id `ConnectParticipant`, endpoint prefix `participant.connect`): CreateParticipantConnection outputs `Websocket`/`ConnectionCredentials`; SendMessage outputs `Id`/`AbsoluteTime`/`MessageMetadata`; SendEvent outputs `Id`/`AbsoluteTime`; GetTranscript is `@paginated(inputToken=NextToken, outputToken=NextToken)` and outputs `InitialContactId`/`Transcript`/`NextToken`; DisconnectParticipant has no output members. `smithy_query` on `Websocket`, `ConnectionCredentials`, `Item` and `StartPosition` confirms the member names/shapes cited above (ISO-8601 `yyyy-MM-ddThh:mm:ss.SSSZ` timestamps, `Item.Type` = MESSAGE|EVENT, `StartPosition{Id, AbsoluteTime, MostRecent}`). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 11 handler files (cancel_participant_authentication.rs, complete_attachment_upload.rs, create_participant_connection.rs, describe_view.rs, disconnect_participant.rs, get_attachment.rs, get_authentication_url.rs, get_transcript.rs, send_event.rs, send_message.rs, start_attachment_upload.rs), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. No handler holds state; there is no test beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST /participant/connection with header `X-Amz-Bearer: <participant-token>` and body {"Type":["CONNECTION_CREDENTIALS","WEBSOCKET"]} — expect 200 with ConnectionCredentials.ConnectionToken and a future ISO-8601 Expiry, plus Websocket.Url/ConnectionExpiry (currently 501)
2. POST /participant/message with header `X-Amz-Bearer: <ConnectionToken from step 1>` and body {"ContentType":"text/plain","Content":"hello","ClientToken":"ct-1"} — expect 200 with a generated Id and AbsoluteTime
3. Repeat the identical SendMessage with ClientToken "ct-1" — expect the SAME Id and AbsoluteTime, and no duplicate transcript item
4. POST /participant/event with the same connection token and {"ContentType":"application/vnd.amazonaws.connect.event.typing"} — expect 200 with Id/AbsoluteTime
5. POST /participant/transcript with the same connection token and {"SortOrder":"ASCENDING","ScanDirection":"FORWARD","MaxResults":1} — expect 200, Transcript containing exactly the "hello" MESSAGE item (Content/ContentType/Id matching step 2), InitialContactId set, and a NextToken
6. POST /participant/transcript again passing that NextToken — expect the typing EVENT item and no repeat of the message
7. POST /participant/transcript with {"MaxResults":500} — expect ValidationException (HTTP 400)
8. POST /participant/message with header `X-Amz-Bearer: bogus-token` — expect AccessDeniedException (HTTP 403)
9. POST /participant/disconnect with the valid connection token — expect 200 with an empty body
10. POST /participant/message again with that now-disconnected connection token — expect AccessDeniedException (HTTP 403)
