Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackagev2-channel-group-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackagev2-channel-group-lifecycle mediapackagev2-channel-lifecycle mediapackagev2-origin-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.mediapackagev2#mediapackagev2
Issue-Key: mediapackagev2|GetChannelGroup|2xx->2xx|$.Description
Issue-Key: mediapackagev2|GetChannelGroup|2xx->2xx|$.ETag
Issue-Key: mediapackagev2|GetChannel|2xx->2xx|$.Description
Issue-Key: mediapackagev2|GetChannel|2xx->2xx|$.ETag
Issue-Key: mediapackagev2|GetOriginEndpoint|2xx->2xx|$.ETag
Issue-Related: 20261022T021034Z-mediapackagev2-channel-and-origin-endpoint-projections-omit-server.md
Issue-Commit: 07653fe6d7ae123674bcbfa9037d42ab6af0ac40

# mediapackagev2 update operations do not persist mutations, so reads return stale description and ETag

> _Note: this issue was identified against the code base at commit `07653fe6d7ae123674bcbfa9037d42ab6af0ac40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateChannelGroup

## Description
A successful mediapackagev2 update is not committed to the emulator's state: the subsequent read of the same resource returns the value the resource had at create time, and the entity tag the read reports is not the one the update response handed out. Every update path in the corpus shows this — channel group, channel and origin endpoint. 3 causes, one per resource type.

Cause 1 of 3: UpdateChannelGroup's Description is not persisted, and GetChannelGroup reports a different ETag than the update returned.
AWS: Smithy: UpdateChannelGroup (PUT /channelGroup/{ChannelGroupName}) takes Description (ResourceDescription, @length 0..1024) and returns Description plus ETag (EntityTag, @length 1..256 @pattern ^[\S]+$); GetChannelGroup returns the same two members. The semantics are the ordinary control-plane read-after-write contract: a 200 update means the new Description is the resource's state, and the ETag returned by the update is the tag a following read observes (that tag is what the model's x-amzn-update-if-match conditional header on UpdateChannelGroup is matched against). The recorded step captured the update's ETag and asserted the read returns it.
Emulator: GetChannelGroup after a 200 UpdateChannelGroup still reports the pre-update Description, and its ETag is a freshly computed value rather than the one the update response carried.
Why: read-after-write is broken for channel groups. A client that updates a description and re-reads sees its write silently lost; worse, optimistic concurrency is unusable — the ETag a client holds from the update never matches what the read reports, so any x-amzn-update-if-match flow built on it either fails or guards nothing.
Fix: the UpdateChannelGroup handler and the channel-group record in mediapackagev2 service state — apply the mutation to the stored resource and derive the ETag from that stored state so update and read agree.
Traces: mediapackagev2-channel-group-lifecycle#task#4 (GetChannelGroup after UpdateChannelGroup).

Cause 2 of 3: UpdateChannel's Description is not persisted, and GetChannel reports a different ETag than the update returned.
AWS: Smithy: UpdateChannel (PUT /channelGroup/{ChannelGroupName}/channel/{ChannelName}/) takes Description and a conditional ETag via httpHeader x-amzn-update-if-match, and returns Description + ETag; GetChannel returns both. Same read-after-write and ETag-continuity contract as Cause 1; the recording asserts the updated description and the update's own ETag on the following read.
Emulator: GetChannel after a 200 UpdateChannel returns the create-time description and an unrelated ETag.
Why: identical client impact to Cause 1 at channel scope — a committed channel edit is invisible to every later read, and the channel's conditional-update token is not stable across operations.
Fix: the UpdateChannel handler and the channel record in mediapackagev2 service state.
Traces: mediapackagev2-channel-lifecycle#task#5 (GetChannel after UpdateChannel).

Cause 3 of 3: GetOriginEndpoint after UpdateOriginEndpoint reports an ETag other than the one the update returned.
AWS: Smithy: UpdateOriginEndpoint (PUT /channelGroup/{ChannelGroupName}/channel/{ChannelName}/originEndpoint/{OriginEndpointName}) accepts the conditional ETag via httpHeader x-amzn-update-if-match and returns ETag; GetOriginEndpoint returns ETag. The entity tag an update hands back is the tag the next read of that endpoint observes — that is what makes the conditional header usable. The recorded step captured the update's ETag and asserted the read returns it.
Emulator: the read's ETag differs from the update's, i.e. the endpoint's tag is recomputed per response instead of tracking committed state.
Why: optimistic concurrency on origin endpoints cannot work: a client that round-trips the ETag from UpdateOriginEndpoint into a following conditional update is using a token the service never acknowledges.
Fix: the UpdateOriginEndpoint handler and the origin-endpoint record in mediapackagev2 service state — ETag must be a function of the stored resource version, not of the response being serialized. (The same cluster also shows the projection gap filed under the mediapackagev2 channel and origin-endpoint projection issue; that is a separate cause and does not cover this one.)
Traces: mediapackagev2-origin-endpoint-lifecycle#task#6 (GetOriginEndpoint after UpdateOriginEndpoint).

The concrete ETag encodings are not a requirement here — only that the tag a read reports is the tag the preceding update returned.

## Evidence
Cause 1 — mediapackagev2-channel-group-lifecycle#task#4, GetChannelGroup, expected 200 / actual 200: $.Description value_mismatch (expected 'updated description', got 'initial description'); $.ETag value_mismatch (expected the ETag captured from the preceding UpdateChannelGroup response, placeholder {{group_etag2}}, got 'd3e5414b87c991b0ec0e505a4d2cee92455c3220966e259c763f08574ed878ba'). The unresolved placeholder is the corpus's way of asserting ETag continuity with the update step.

Cause 2 — mediapackagev2-channel-lifecycle#task#5, GetChannel, expected 200 / actual 200: $.Description value_mismatch (expected 'probe channel updated', got 'probe channel'); $.ETag value_mismatch (expected {{channel_etag2}}, i.e. the UpdateChannel response's ETag, got '2d42d51685f4b3eee366694b6335f005621bb26463cfac456ef02a9cd3a53b28').

Cause 3 — mediapackagev2-origin-endpoint-lifecycle#task#6, GetOriginEndpoint, expected 200 / actual 200: $.ETag value_mismatch (expected {{endpoint_etag2}}, i.e. the UpdateOriginEndpoint response's ETag, got '9d5e54dc31b4bf03a195548fe8134885e15d15427b79f1d45910def35f3c7558'). StartoverWindowSeconds 3600 (the updated value) is also absent from the body, which is the projection defect filed separately.

state_reset_failures was 0 for this replay, so no fixture ran against a previous fixture's state and these stale reads are not a harness artifact.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus); trace set simparity-wire-aws-v1.
2. Cause 1: fixture mediapackagev2-channel-group-lifecycle — CreateChannelGroup with a description, UpdateChannelGroup with a new description, then step task#4 GetChannelGroup; compare Description and ETag with the update response.
3. Cause 2: fixture mediapackagev2-channel-lifecycle — CreateChannel, UpdateChannel with a new description, then step task#5 GetChannel; compare Description and ETag with the update response.
4. Cause 3: fixture mediapackagev2-origin-endpoint-lifecycle — CreateOriginEndpoint, UpdateOriginEndpoint, then step task#6 GetOriginEndpoint; compare ETag with the UpdateOriginEndpoint response's ETag.
