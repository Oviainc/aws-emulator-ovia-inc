Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Amazon Interactive Video Service (IVS) emulator

_Filed: 2026-10-21T21:08:48Z_
_Operation: service-com-amazonaws-ivs-amazoninteractivevideoservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 41 operations in `services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no IVS client can do anything against the emulator. Implement a FIRST, SMALL slice: the channel lifecycle, which is the service's primary resource and the entry point of every real IVS workflow (restJson1, sdkId `ivs`, sigv4 name `ivs`).

### Happy path to implement (in order)

1. **CreateChannel** — `POST /CreateChannel`. Input: `name` (0..128, `^[a-zA-Z0-9-_]*$`), `latencyMode` (LOW|NORMAL, default LOW), `type` (BASIC|STANDARD|ADVANCED_SD|ADVANCED_HD, default STANDARD), `authorized` (default false), `insecureIngest` (default false), `preset`, `recordingConfigurationArn`, `playbackRestrictionPolicyArn`, `adConfigurationArn`, `containerFormat` (default TS), `tags`. Must MINT and PERSIST a new channel: generate `arn` matching `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:channel/[a-zA-Z0-9-]+$`, plus `ingestEndpoint`, `playbackUrl`, and apply the modeled defaults for any member the caller omitted. Must also mint and persist an initial stream key (`arn` matching `^arn:aws:ivs:...:stream-key/[a-zA-Z0-9-]+$`, `value`, `channelArn`) and return BOTH `channel` and `streamKey` in the output.
2. **GetChannel** — `POST /GetChannel`, input `arn` (required). Must look the channel up in persisted state and return the stored `channel` byte-for-byte equal to what CreateChannel returned (same arn, name, type, latencyMode, authorized, insecureIngest, ingestEndpoint, playbackUrl, tags). Unknown/never-created arn ⇒ `ResourceNotFoundException` (HTTP 404), malformed arn ⇒ `ValidationException` (HTTP 400).
3. **ListChannels** — `POST /ListChannels`. Must return one `ChannelSummary` (arn, name, latencyMode, authorized, recordingConfigurationArn, insecureIngest, type, preset, playbackRestrictionPolicyArn, adConfigurationArn, tags) per persisted channel — derived from state, not a fixed list. Honour `maxResults` (1..100) by truncating and emitting an opaque `nextToken` (`^[a-zA-Z0-9+/=_-]*$`) that resumes exactly where the previous page stopped, and honour `filterByName` (exact-name filter).
4. **UpdateChannel** — `POST /UpdateChannel`, input `arn` (required) + any mutable member (`name`, `latencyMode`, `type`, `authorized`, `insecureIngest`, `preset`, `recordingConfigurationArn`, `playbackRestrictionPolicyArn`, `adConfigurationArn`, `containerFormat`). Must MUTATE the stored channel in place, leave omitted members untouched, and return the updated `channel`; a following GetChannel must observe the new values. Unknown arn ⇒ `ResourceNotFoundException` (404).
5. **CreateStreamKey / ListStreamKeys** — `CreateStreamKey` takes `channelArn` (required) and must mint + persist an additional stream key bound to that channel (404 if the channel does not exist); `ListStreamKeys` takes `channelArn` (required) and must return the `StreamKeySummary` list for exactly that channel from state, including the key minted by CreateChannel, with `maxResults` (1..50)/`nextToken` honoured.
6. **DeleteChannel** — `POST /DeleteChannel -> 204`, input `arn` (required). Must remove the channel AND its stream keys from state, answer 204 with an empty body, and make the resource observably gone: a subsequent GetChannel ⇒ `ResourceNotFoundException` (404), ListChannels no longer lists it, and a second DeleteChannel of the same arn ⇒ 404.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the request back, no minting an ARN that is never stored. A create followed by a read must return what was created; a delete followed by a read must 404.

## State Model

Store in `ComAmazonawsIvsAmazoninteractivevideoserviceState` (reached via `crate::state::service_state(&*self.engine)`, one shared engine per crate):

- `channels: BTreeMap<String /* channel ARN */, Channel>` where `Channel { arn, name, latency_mode, type_, authorized, insecure_ingest, preset, container_format, recording_configuration_arn, playback_restriction_policy_arn, ad_configuration_arn, ingest_endpoint, playback_url, srt: Option<Srt{endpoint, passphrase}>, multitrack_input_configuration, tags: BTreeMap<String,String> }` — BTreeMap so ListChannels has a stable order for token-based paging.
- `stream_keys: BTreeMap<String /* stream-key ARN */, StreamKeyRecord>` where `StreamKeyRecord { arn, value, channel_arn, tags }`; ListStreamKeys filters by `channel_arn`, and DeleteChannel removes every record whose `channel_arn` matches.
- `account_id: String` and `region: String` (or a single `arn_prefix`) used to mint ARNs that satisfy the modeled ARN patterns consistently across operations.
- An id counter / random-suffix source for channel ids, stream-key ids and stream-key `value`s, so two CreateChannel calls never collide.

All other IVS operations (recording configurations, playback key pairs, playback restriction policies, ad configurations, streams/stream sessions, GetStream, StopStream, PutMetadata, InsertAdBreak, viewer-session revocation, Batch*, Tag/Untag/ListTagsForResource) stay at 501 for now and are tracked as individual gap issues later.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/ops/` matches all 41 operation files (create_channel.rs, get_channel.rs, list_channels.rs, update_channel.rs, create_stream_key.rs, list_stream_keys.rs, delete_channel.rs, ...), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501 with zero persisted state. Contracts above are taken from the crate's generated OPERATIONS.md (routes, required members, constraint patterns, defaults, modeled errors incl. `ResourceNotFoundException` 404 / `ValidationException` 400 and the `-> 204` empty-output deletes) and from the Smithy model shapes `Channel`, `ChannelSummary`, `StreamKey`, `StreamKeySummary` (members arn/name/latencyMode/type/authorized/insecureIngest/ingestEndpoint/playbackUrl/tags, and arn/value/channelArn/tags). No tests exercise these paths today; the crate ships only `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an IVS client at it (`aws ivs --endpoint-url <emulator>`).
2. `CreateChannel {"name":"demo-channel","type":"STANDARD","latencyMode":"LOW","tags":{"env":"test"}}` — expect 200 with a `channel` carrying a fresh `arn`, `ingestEndpoint`, `playbackUrl` and the echoed-but-stored name/type/tags, plus a `streamKey` with its own arn and value. Today: HTTP 501 NotImplemented.
3. `GetChannel {"arn":"<arn from step 2>"}` — expect 200 with the identical channel fields read from state.
4. `ListChannels {}` — expect the new channel present as a ChannelSummary; repeat with `maxResults:1` across two pages via `nextToken` and expect no duplicates and no omissions.
5. `CreateStreamKey {"channelArn":"<arn>"}` then `ListStreamKeys {"channelArn":"<arn>"}` — expect BOTH the key minted by CreateChannel and the new one, each with `channelArn` equal to the channel arn.
6. `UpdateChannel {"arn":"<arn>","name":"demo-renamed","authorized":true}` then `GetChannel` — expect `name=demo-renamed`, `authorized=true`, and `type`/`latencyMode` unchanged.
7. `DeleteChannel {"arn":"<arn>"}` — expect HTTP 204 empty body; then `GetChannel {"arn":"<arn>"}` ⇒ ResourceNotFoundException (404), `ListChannels {}` no longer contains it, and `ListStreamKeys {"channelArn":"<arn>"}` ⇒ 404.
8. `GetChannel {"arn":"arn:aws:ivs:us-east-1:123456789012:channel/doesnotexist"}` on a fresh store ⇒ ResourceNotFoundException (404), and `GetChannel {"arn":"not-an-arn"}` ⇒ ValidationException (400).
