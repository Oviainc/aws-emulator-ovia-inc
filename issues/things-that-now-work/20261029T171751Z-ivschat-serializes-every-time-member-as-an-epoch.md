Issue-Source: trace-sweep-divergence
Issue-Trace: ivschat-chat-token-and-messaging
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivschat-chat-token-and-messaging ivschat-list-rooms-filtering ivschat-room-lifecycle ivschat-tagging-workflow
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat|CreateRoom|2xx->2xx|$.createTime
Issue-Key: ivschat|CreateRoom|2xx->2xx|$.updateTime
Issue-Key: ivschat|CreateChatToken|2xx->2xx|$.sessionExpirationTime
Issue-Key: ivschat|CreateChatToken|2xx->2xx|$.token
Issue-Key: ivschat|CreateChatToken|2xx->2xx|$.tokenExpirationTime
Issue-Key: ivschat|ListRooms|2xx->2xx|$.rooms[arn="arn
Issue-Key: ivschat|GetRoom|2xx->2xx|$.createTime
Issue-Key: ivschat|GetRoom|2xx->2xx|$.updateTime
Issue-Key: ivschat|UpdateRoom|2xx->2xx|$.createTime
Issue-Key: ivschat|UpdateRoom|2xx->2xx|$.updateTime
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# ivschat serializes every Time member as an epoch number instead of an ISO 8601 date-time string

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoom

## Description
Five net-new causes sharing one fault: every ivschat output member targeting the Time shape is written to the wire as a floating-point epoch-seconds number, where the model pins it to the date-time (ISO 8601) form. ivschat is restJson1 (Smithy: protocols=[restJson1]), and the Time shape carries @timestampFormat date-time on each of these members, so the serialized value must be an ISO 8601 string such as 2026-09-06T02:52:29.837Z — the model's own doc on CreateRoomResponse.createTime says "This is an ISO 8601 timestamp; note that this is returned as a string". The emulator instead emits a JSON number of epoch seconds with fractional part. This is one serializer defect reached through five operations; it is not covered by any open ivschat issue (the only open one, 20261023T054149Z-ivschat-logging-configuration-fidelity-missing-s3-destination-validation, concerns logging-configuration validation and error message templates).

Cause 1 of 5: CreateRoom.createTime and CreateRoom.updateTime.
AWS: CreateRoomResponse.createTime and .updateTime both target Time with @timestampFormat date-time (Smithy: query='operation' CreateRoom output). A fresh room reports both as the same ISO 8601 instant, to millisecond precision, with a trailing Z.
Emulator: both members are JSON numbers (epoch seconds with a fractional part) rather than date-time strings.
Why: an SDK that parses the field per the model fails to deserialize a number as a date-time, or (where it is lenient) a client reading the raw JSON gets a number where AWS documents a string — so any client that formats, compares lexicographically, or stores the room's creation time breaks on the type, not merely the value.
Fix: the ivschat crate's restJson1 output serializer for Time-targeting members (the CreateRoom response builder path), which must render date-time rather than epoch-seconds.
Traces: ivschat-chat-token-and-messaging#task#1, plus ivschat-list-rooms-filtering#task#1, ivschat-list-rooms-filtering#task#2, ivschat-room-lifecycle#task#1, ivschat-tagging-workflow#setup#1 in the same cluster.

Cause 2 of 5: CreateChatToken.tokenExpirationTime and .sessionExpirationTime.
AWS: both target Time with @timestampFormat date-time (Smithy: CreateChatToken output = token (sensitive), tokenExpirationTime, sessionExpirationTime). They are ISO 8601 strings; the recording shows second precision here, derived from the request's sessionDurationInMinutes (range 1..180) and the token's own validity window — the instants themselves are clock values, only the date-time ENCODING is the requirement.
Emulator: both are epoch-seconds numbers.
Why: a chat client decides when to re-mint a token from these two fields; a number where a date-time string is modeled makes the expiry unparseable.
Fix: the same Time serializer, on the CreateChatToken response builder. (The step also reports $.token; the corpus marks that value '<shaped-not-asserted>' — it is a per-run minted credential, so its literal value is not part of this issue.)
Traces: ivschat-chat-token-and-messaging#task#2.

Cause 3 of 5: ListRooms rooms[].createTime and rooms[].updateTime.
AWS: each RoomSummary in ListRoomsResponse.rooms carries the same Time members in date-time form (Smithy: ListRooms output rooms -> RoomList, @required).
Emulator: every element's createTime/updateTime is an epoch-seconds number, so the defect is in the list projection too, not only the single-room responses.
Why: as Cause 1, multiplied across a page of rooms; a client sorting a listing by creation time sees a different type than AWS documents.
Fix: the same Time serializer, reached through the RoomSummary projection used by ListRooms.
Traces: ivschat-list-rooms-filtering#task#3, plus ivschat-list-rooms-filtering#task#4 and ivschat-room-lifecycle#task#5 in the same cluster.

Cause 4 of 5: GetRoom.createTime and GetRoom.updateTime.
AWS: GetRoomResponse carries the same two date-time Time members.
Emulator: epoch-seconds numbers.
Why: as Cause 1.
Fix: the same Time serializer on the GetRoom response builder.
Traces: ivschat-room-lifecycle#task#2, plus ivschat-room-lifecycle#task#4 in the same cluster.

Cause 5 of 5: UpdateRoom.createTime and UpdateRoom.updateTime.
AWS: UpdateRoomResponse carries the same two date-time Time members; after an update, createTime keeps the room's original instant while updateTime advances to the instant of the update, so the two differ.
Emulator: both are epoch-seconds numbers (and the pair does differ, so only the encoding is wrong here).
Why: as Cause 1; a client that detects "was this room modified" by comparing the two strings cannot parse either.
Fix: the same Time serializer on the UpdateRoom response builder.
Traces: ivschat-room-lifecycle#task#3.

## Evidence
Replay of simparity-wire-aws-v1. All five causes return the correct HTTP 200; only the timestamp ENCODING diverges. The expected literals below are one recording's clock values and are not themselves requirements — the requirement is the ISO 8601 date-time string form (trailing Z) instead of a JSON number.

Cause 1 — ivschat-chat-token-and-messaging#task#1, CreateRoom, 200 -> 200:
  $.createTime: value_mismatch (expected '2026-09-06T02:52:29.837Z', got 1793294084.896)
  $.updateTime: value_mismatch (expected '2026-09-06T02:52:29.837Z', got 1793294084.896)
  Cluster also covers ivschat-list-rooms-filtering#task#1, ivschat-list-rooms-filtering#task#2, ivschat-room-lifecycle#task#1, ivschat-tagging-workflow#setup#1 (5 steps).

Cause 2 — ivschat-chat-token-and-messaging#task#2, CreateChatToken, 200 -> 200:
  $.sessionExpirationTime: value_mismatch (expected '2026-09-06T03:52:29Z', got 1793297684.924)
  $.tokenExpirationTime: value_mismatch (expected '2026-09-06T03:52:29Z', got 1793294144.924)
  $.token: expected '<shaped-not-asserted>' — a per-run minted credential, excluded from this issue.

Cause 3 — ivschat-list-rooms-filtering#task#3, ListRooms, 200 -> 200:
  $.rooms[arn="arn:aws:ivschat:us-west-2:000000000000:room/0AHajh7wQKfu"].createTime: value_mismatch (expected '2026-09-06T02:50:04.585Z', got 1793294086.377)
  $.rooms[...].updateTime: value_mismatch (expected '2026-09-06T02:50:04.585Z', got 1793294086.377)
  Cluster also covers ivschat-list-rooms-filtering#task#4 and ivschat-room-lifecycle#task#5 (3 steps).

Cause 4 — ivschat-room-lifecycle#task#2, GetRoom, 200 -> 200:
  $.createTime: value_mismatch (expected '2026-09-06T02:50:02.583Z', got 1793294086.998)
  $.updateTime: value_mismatch (expected '2026-09-06T02:50:02.583Z', got 1793294086.998)
  Cluster also covers ivschat-room-lifecycle#task#4 (2 steps).

Cause 5 — ivschat-room-lifecycle#task#3, UpdateRoom, 200 -> 200:
  $.createTime: value_mismatch (expected '2026-09-06T02:50:02.583Z', got 1793294086.998)
  $.updateTime: value_mismatch (expected '2026-09-06T02:50:02.768Z', got 1793294087.019)

Model basis: Smithy query='operation' on CreateRoom / ListRooms / CreateChatToken shows createTime, updateTime, tokenExpirationTime and sessionExpirationTime all targeting Time with timestampFormat 'date-time'; CreateRoomResponse.createTime's doc states the value is an ISO 8601 timestamp returned as a string.

## Steps to Reproduce
1. Boot the emulator and replay fixtures ivschat-chat-token-and-messaging, ivschat-list-rooms-filtering and ivschat-room-lifecycle from simparity-wire-aws-v1.
2. Cause 1 — ivschat.create_room(name=<valid name>) and read the RAW response JSON. createTime and updateTime must be ISO 8601 date-time strings ending in Z; the emulator emits JSON numbers of epoch seconds. (ivschat-chat-token-and-messaging#task#1)
3. Cause 2 — ivschat.create_chat_token(roomIdentifier=<the room's arn>, userId=<any>, sessionDurationInMinutes=<1..180>) and read the raw JSON: tokenExpirationTime and sessionExpirationTime must be ISO 8601 date-time strings; the emulator emits epoch numbers. Ignore the token value itself. (ivschat-chat-token-and-messaging#task#2)
4. Cause 3 — ivschat.list_rooms() and read the raw JSON: every rooms[].createTime / rooms[].updateTime must be an ISO 8601 string; the emulator emits epoch numbers. (ivschat-list-rooms-filtering#task#3)
5. Cause 4 — ivschat.get_room(identifier=<the room's arn>): same two members, same divergence. (ivschat-room-lifecycle#task#2)
6. Cause 5 — ivschat.update_room(identifier=<the room's arn>, maximumMessageLength=<new value>): createTime keeps the original instant and updateTime advances, but both are emitted as epoch numbers rather than ISO 8601 strings. (ivschat-room-lifecycle#task#3)
