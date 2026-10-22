Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the Chime SDK Messaging emulator

_Filed: 2026-10-21T21:18:52Z_
_Operation: service-com-amazonaws-chimesdkmessaging-chimemessagingservice | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 51 operations in `services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client workflow can run. Implement a FIRST, SMALL stateful slice: the channel-messaging happy path (restJson1, sigv4 signing name `chime`, endpoint prefix `messaging-chime`).

### Happy path to implement (in order)

1. **CreateChannel** — `POST /channels` -> 201. Input: `AppInstanceArn` (required), `Name` (required), `ClientRequestToken` (required, @idempotencyToken), `ChimeBearer` (required, header `x-amz-chime-bearer`), optional `Mode` (RESTRICTED|UNRESTRICTED), `Privacy` (PRIVATE|PUBLIC), `Metadata`, `ChannelId`. Output: `ChannelArn`. Must MINT and PERSIST the channel: derive the ARN from `AppInstanceArn` + `/channel/` + (`ChannelId` or a generated UUID), store `Name`, `Mode` (default UNRESTRICTED), `Privacy` (default PUBLIC), `Metadata`, `CreatedBy` (Identity built from `ChimeBearer`), `CreatedTimestamp`, `LastUpdatedTimestamp`. Repeating the same `ClientRequestToken` must return the same `ChannelArn` rather than a second channel.
2. **SendChannelMessage** — `POST /channels/{ChannelArn}/messages` -> 201. Input: `ChannelArn` (@httpLabel), `Content` (required), `Type` (CONTROL|STANDARD, required), `Persistence` (PERSISTENT|NON_PERSISTENT, required), `ClientRequestToken` (required), `ChimeBearer` (required), optional `Metadata`, `ContentType`, `MessageAttributes`. Output: `ChannelArn`, `MessageId`, `Status` (`ChannelMessageStatusStructure{Value: SENT}`). Must APPEND a message record to the stored channel (generating a `MessageId` matching `^[-_a-zA-Z0-9]*$`), set `CreatedTimestamp`/`Sender`, and update the channel's `LastMessageTimestamp` for PERSISTENT messages. Unknown `ChannelArn` must not silently succeed.
3. **GetChannelMessage** — `GET /channels/{ChannelArn}/messages/{MessageId}` -> 200. Output: `ChannelMessage` populated FROM the stored message: `ChannelArn`, `MessageId`, `Content`, `Metadata`, `Type`, `Persistence`, `CreatedTimestamp`, `LastUpdatedTimestamp`, `Sender`, `Redacted` (default false), `Status`, `MessageAttributes`, `ContentType`. Unknown `MessageId` -> modeled `NotFoundException` (HTTP 404).
4. **ListChannelMessages** — `GET /channels/{ChannelArn}/messages` -> 200. Output: `ChannelArn`, `ChannelMessages` (list<ChannelMessageSummary> built from stored messages), `NextToken`. Must honour `max-results` (1..50), `sort-order` (ASCENDING|DESCENDING, default DESCENDING by `CreatedTimestamp`), `not-before`/`not-after` filters, and emit/accept a real opaque `NextToken` that resumes where the previous page stopped.
5. **DescribeChannel** — `GET /channels/{ChannelArn}` -> 200. Output: `Channel` reflecting exactly what CreateChannel stored, with `LastMessageTimestamp` advanced by step 2.
6. **DeleteChannel** — `DELETE /channels/{ChannelArn}` -> 204. Must REMOVE the channel and its messages so a subsequent DescribeChannel/ListChannelMessages no longer sees it, and ListChannels no longer lists it.

A 7th op worth including because it is nearly free once the store exists: **ListChannels** — `GET /channels?app-instance-arn=...` -> 200, returning `Channels` (list<ChannelSummary>: `Name`, `ChannelArn`, `Mode`, `Privacy`, `Metadata`, `LastMessageTimestamp`) filtered by `AppInstanceArn` and optional `privacy`, with `max-results`/`next-token` pagination.

Every one of these must compute its response from persisted state. NO canned bodies, NO echoing the request back, NO minting an ARN/MessageId that is never stored. A create followed by a describe/get must return what was created; a delete must be observable by the next read.

## State Model

Store in `ComAmazonawsChimesdkmessagingChimemessagingserviceState` (src/state.rs), reached via `crate::state::service_state(&*self.engine)`:

- `channels: HashMap<String /* ChannelArn */, Channel>` where
  `Channel { arn, app_instance_arn, channel_id, name, mode: ChannelMode, privacy: ChannelPrivacy, metadata: Option<String>, created_by: Identity /* from ChimeBearer */, created_timestamp, last_updated_timestamp, last_message_timestamp: Option<Timestamp>, elastic_channel_configuration: Option<..>, expiration_settings: Option<..>, tags: Vec<Tag>, messages: Vec<ChannelMessageRecord>, next_message_seq: u64 }`
- `ChannelMessageRecord { message_id, content, content_type: Option<String>, metadata: Option<String>, r#type: ChannelMessageType, persistence: ChannelMessagePersistenceType, created_timestamp, last_updated_timestamp, last_edited_timestamp: Option<Timestamp>, sender: Identity, redacted: bool /* default false */, status: ChannelMessageStatus /* SENT */, message_attributes: HashMap<String, MessageAttributeValue>, sub_channel_id: Option<String>, target: Option<Vec<Target>> }`
- `create_channel_idempotency: HashMap<String /* ClientRequestToken */, String /* ChannelArn */>` and `message_idempotency: HashMap<(ChannelArn, ClientRequestToken), MessageId>` so repeated @idempotencyToken requests return the first result.
- `list_tokens`: pagination cursors may be an encoded offset/last-key string rather than a stored map, but a `NextToken` returned must resume the same ordered view.

Out of scope for this first slice (stays 501, filed later as individual gap issues): channel flows (Create/Update/Delete/Associate/DisassociateChannelFlow, ChannelFlowCallback), memberships/moderators/bans, membership preferences, streaming configurations, SearchChannels, ListSubChannels, Redact/Update/DeleteChannelMessage, UpdateChannelReadMarker, PutChannelExpirationSettings, Tag/Untag/ListTagsForResource, GetMessagingSessionEndpoint.

## Evidence

`services/service-com-amazonaws-chimesdkmessaging-chimemessagingservice/OPERATIONS.md` (51 operations, protocol restJson1) documents each route and member cited above: CreateChannel `POST /channels -> 201` with required `AppInstanceArn`/`Name`/`ClientRequestToken`/`ChimeBearer` and output `ChannelArn` (lines 47-52); SendChannelMessage `POST /channels/{ChannelArn}/messages -> 201` with required `Content`/`Type`/`Persistence`/`ClientRequestToken` and output `ChannelArn`/`MessageId`/`Status` (lines 345-350); GetChannelMessage `GET /channels/{ChannelArn}/messages/{MessageId} -> 200` -> `ChannelMessage`, with `NotFoundException (HTTP 404)` modeled (lines 194-199); ListChannelMessages `GET /channels/{ChannelArn}/messages -> 200`, @paginated(inputToken=NextToken, outputToken=NextToken), `max-results` range 1..50, `sort-order`, `not-before`/`not-after` (lines 254-260); DescribeChannel `GET /channels/{ChannelArn} -> 200` -> `Channel` (lines 131-136); DeleteChannel `DELETE /channels/{ChannelArn} -> 204` (lines 82-87); ListChannels `GET /channels -> 200` -> `Channels`/`NextToken` (lines 270-276). Shape members for the state model come from the Smithy model shapes `Channel`, `ChannelMessage`, `ChannelMessageSummary`, `ChannelSummary`, `ChannelMessageStatusStructure` (smithy_query). Proof of the stub state: `src/ops/create_channel.rs` carries `//! SCAFFOLDED, NOT IMPLEMENTED:` (line 4) and its `handle` ends in `Err(crate::scaffolded::Scaffolded::not_implemented("CreateChannel"))` (line 205); all 51 op files in `src/ops/` are the same generated scaffold. No tests exist beyond `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `chime-sdk-messaging` client at it.
2. CreateChannel with AppInstanceArn=arn:aws:chime:us-east-1:111122223333:app-instance/ai1, Name="demo", ClientRequestToken="tok-1", header x-amz-chime-bearer=arn:aws:chime:us-east-1:111122223333:app-instance/ai1/user/alice. Expect 201 and a ChannelArn; today: 501 NotImplemented.
3. SendChannelMessage to that ChannelArn with Content="hello", Type=STANDARD, Persistence=PERSISTENT, ClientRequestToken="tok-2". Expect 201 with MessageId and Status.Value=SENT.
4. GetChannelMessage for that ChannelArn+MessageId. Expect 200 and ChannelMessage.Content=="hello", Sender == the bearer identity, Redacted==false, CreatedTimestamp set.
5. ListChannelMessages for the channel with max-results=1 after sending 3 messages. Expect 1 summary plus a NextToken; following the token returns the remaining messages with no duplicates, ordered per sort-order.
6. DescribeChannel for the ChannelArn. Expect 200 with Name=="demo", Privacy/Mode defaults, and LastMessageTimestamp at/after the send.
7. ListChannels with app-instance-arn=arn:aws:chime:us-east-1:111122223333:app-instance/ai1. Expect the channel in Channels.
8. DeleteChannel for the ChannelArn. Expect 204; a following DescribeChannel/ListChannelMessages no longer returns it and ListChannels omits it.
9. Repeat CreateChannel with ClientRequestToken="tok-1": expect the same ChannelArn, not a duplicate channel.
