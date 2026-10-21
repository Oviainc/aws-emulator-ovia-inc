Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4

# Bootstrap the ivschat emulator

_Filed: 2026-10-21T21:47:46Z_
_Operation: service-com-amazonaws-ivschat-amazoninteractivevideoservicechat | Class: stub_

> _Note: this issue was identified against the code base at commit `a2986bf0fd3cc35fe32946cf5d2b8a785e7f69a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 17 operations in this crate is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding zero state, so no client workflow works at all. Implement a FIRST, SMALL slice: the chat Room lifecycle plus a token issue, all computed from persisted state.

Happy path to implement, in order (fields from OPERATIONS.md):

1. **CreateRoom** (`POST /CreateRoom`, restJson1) — input `name` (<=128, `^[a-zA-Z0-9-_]*$`), `maximumMessageRatePerSecond` (1..100), `maximumMessageLength` (1..500), `messageReviewHandler`, `tags`, `loggingConfigurationIdentifiers` (0..3). Must mint and STORE a room: generate `id` (exactly 12 chars, `^[a-zA-Z0-9]+$`), build `arn` as `arn:aws:ivschat:<region>:<account>:room/<id>` (matching the modeled pattern), set `createTime`/`updateTime` to now, apply AWS defaults for omitted knobs (`maximumMessageRatePerSecond` = 10, `maximumMessageLength` = 500), and echo back only what was persisted. Returns the full room shape.

2. **GetRoom** (`POST /GetRoom`, @readonly) — input `identifier` (room ARN, required). Must look the room up in state by ARN (also accept the bare id) and return the stored `arn`, `id`, `name`, `createTime`, `updateTime`, `maximumMessageRatePerSecond`, `maximumMessageLength`, `messageReviewHandler`, `tags`, `loggingConfigurationIdentifiers`. Unknown identifier => `ResourceNotFoundException` (HTTP 404); malformed/non-ARN identifier => `ValidationException` (HTTP 400). Must NOT write state.

3. **ListRooms** (`POST /ListRooms`, @paginated) — input `name`, `nextToken` (0..1024), `maxResults` (1..50), `messageReviewHandlerUri`, `loggingConfigurationIdentifier`. Must enumerate rooms from state into `rooms` (list<RoomSummary>, required — always present, empty list when none), honour the `name` and `loggingConfigurationIdentifier` filters, cap the page at `maxResults`, and emit an opaque `nextToken` that a follow-up call resumes from without repeating or dropping rooms.

4. **UpdateRoom** (`POST /UpdateRoom`) — input `identifier` (required) plus optional `name`, `maximumMessageRatePerSecond`, `maximumMessageLength`, `messageReviewHandler`, `loggingConfigurationIdentifiers`. Must mutate the STORED room in place (only the members present in the request), bump `updateTime`, leave `arn`/`id`/`createTime`/`tags` unchanged, and return the post-update room. A subsequent GetRoom must observe the change. Unknown identifier => `ResourceNotFoundException`.

5. **CreateChatToken** (`POST /CreateChatToken`) — input `roomIdentifier` (room ARN, required), `userId` (1..128, required), `capabilities`, `sessionDurationInMinutes` (1..180), `attributes`. Must verify the room exists in state (404 `ResourceNotFoundException` otherwise), then mint an opaque `token` recorded against (room, userId, capabilities, attributes) in state, with `tokenExpirationTime` = now + 1 minute and `sessionExpirationTime` = now + `sessionDurationInMinutes` (default 60). The token must not be a constant: two calls yield different tokens.

6. **DeleteRoom** (`POST /DeleteRoom` -> HTTP 204) — input `identifier` (required). Must REMOVE the room (and its tokens) from state and return an empty 204 body. Unknown identifier => `ResourceNotFoundException` (404). After deletion, GetRoom must 404 and ListRooms must not include the room.

None of these may return a canned, hardcoded or purely echoed response: every response field must be read back out of the stored room record, and ids/ARNs/timestamps must be generated once at create time and remain stable across later reads. A create followed by a get must return exactly what was created.

## State Model

In `src/state.rs` (`ComAmazonawsIvschatAmazoninteractivevideoservicechatState`, reached via `crate::state::service_state(&*self.engine)`):

- `rooms: BTreeMap<String /* room id */, Room>` — insertion/created-time ordering preserved for deterministic ListRooms paging, with a lookup helper that resolves either a room ARN or a bare id to the id key.
  - `Room { id: String /* 12 chars */, arn: String, name: Option<String>, create_time: SystemTime, update_time: SystemTime, maximum_message_rate_per_second: i32 /* default 10 */, maximum_message_length: i32 /* default 500 */, message_review_handler: Option<MessageReviewHandler>, logging_configuration_identifiers: Vec<String> /* <=3 */, tags: BTreeMap<String, String> /* <=50 */ }`
- `chat_tokens: BTreeMap<String /* token */, ChatToken>`
  - `ChatToken { token: String, room_id: String, user_id: String, capabilities: Vec<String>, attributes: BTreeMap<String, String>, token_expiration_time: SystemTime, session_expiration_time: SystemTime }`
- `next_id: u64` or an RNG seed used to mint the 12-character `^[a-zA-Z0-9]+$` ids, so ids are unique per store and never reused after a delete.

Out of scope for this first slice (stay 501, filed separately later): SendEvent, DeleteMessage, DisconnectUser, Create/Get/Update/Delete/ListLoggingConfigurations, TagResource, UntagResource, ListTagsForResource.

## Evidence

OPERATIONS.md for this crate (services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat/OPERATIONS.md) lists all 17 ivschat operations with their routes, input/output members, constraints and errors — CreateRoom (lines 39-44), GetRoom (81-87), ListRooms (97-103), UpdateRoom (142-147), CreateChatToken (25-30), DeleteRoom (60-65). `grep -r '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 17 handler files (create_room.rs, get_room.rs, list_rooms.rs, update_room.rs, create_chat_token.rs, delete_room.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501. No handler touches `crate::state::service_state`, so the service holds zero state. The only test present is the generated tests/state_roundtrip.rs example; there is no behavioral coverage.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI ivschat client (restJson1, signing name `ivschat`) at it.
2. `CreateRoom {"name":"demo-room","maximumMessageLength":200}` — expect HTTP 200 with a 12-char `id`, an `arn` of the form `arn:aws:ivschat:<region>:<account>:room/<id>`, `createTime`/`updateTime`, `maximumMessageLength` 200 and the default `maximumMessageRatePerSecond`; actual: HTTP 501 NotImplemented.
3. `GetRoom {"identifier":"<arn from step 2>"}` — expect the identical room record back from state; actual: HTTP 501.
4. `ListRooms {"maxResults":1}` — expect `rooms` containing the created room and an opaque `nextToken` when more rooms exist; actual: HTTP 501.
5. `UpdateRoom {"identifier":"<arn>","maximumMessageRatePerSecond":5}` then `GetRoom` — expect the stored room to show rate 5, a bumped `updateTime` and an unchanged `createTime`/`id`/`arn`; actual: HTTP 501.
6. `CreateChatToken {"roomIdentifier":"<arn>","userId":"alice","capabilities":["SEND_MESSAGE"],"sessionDurationInMinutes":60}` — expect a non-empty unique `token` plus `tokenExpirationTime` and `sessionExpirationTime`; actual: HTTP 501.
7. `CreateChatToken` with a room ARN that was never created — expect HTTP 404 `ResourceNotFoundException`; actual: HTTP 501.
8. `DeleteRoom {"identifier":"<arn>"}` — expect HTTP 204 with empty body; then `GetRoom` on the same ARN must return HTTP 404 `ResourceNotFoundException` and `ListRooms` must omit it; actual: HTTP 501 for all three.
