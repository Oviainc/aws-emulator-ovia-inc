Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-batch-get-channel-and-stream-key
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-batch-get-channel-and-stream-key ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].failoverSlateConfigurationArn
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].ingestEndpoint
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].srt.endpoint
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn
Issue-Commit: 3cc945a55a02d2e7b9c7c43c647d43587979fb8a

# IVS channel projections still omit failoverSlateConfigurationArn on reads

> _Note: this issue was identified against the code base at commit `3cc945a55a02d2e7b9c7c43c647d43587979fb8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetChannel

## Description
Every IVS response that projects a channel — the full `Channel` shape BatchGetChannel returns and the `ChannelSummary` ListChannels returns — still drops the `failoverSlateConfigurationArn` member AWS emits as the empty string when no failover slate is configured. THREE net-new causes, one per failing fixture step; each must be closed before submitting.

Shared AWS behavior: `failoverSlateConfigurationArn` is not present in this model revision of com.amazonaws.ivs (Smithy: `Channel` and `ChannelSummary` carry arn/name/latencyMode/type/recordingConfigurationArn/ingestEndpoint/playbackUrl/authorized/tags/insecureIngest/preset/srt/playbackRestrictionPolicyArn/multitrackInputConfiguration/containerFormat/adConfigurationArn and NO failover member), so this is RECORDED BEHAVIOR ONLY: the recorded 200 responses carry `failoverSlateConfigurationArn` on every projected channel, with the value `""` when the channel has no failover slate — exactly the way the model's other "Default: \"\"" ARN members (`recordingConfigurationArn`, `playbackRestrictionPolicyArn`, `adConfigurationArn`) are emitted rather than omitted. The requirement is the KEY's presence with the empty string for an unconfigured channel; a configured channel would carry its slate ARN.

Shared emulator behavior: the member is absent (`got None`) from every channel object the emulator serves — the ivs crate contains no occurrence of `failoverSlate` at all (services/service-com-amazonaws-ivs-amazoninteractivevideoservice/src/state.rs holds the shared channel rules and mints/echoes only the modeled members, cf. `effective_multitrack_input_configuration` at state.rs:609).

Shared why: client code that decides "is a failover slate configured?" by comparing `failoverSlateConfigurationArn` to `""` gets a missing key / null instead, so the comparison throws or silently reads as "unknown"; code that round-trips a channel read into an update drops the member entirely.

Shared fix: the Channel / ChannelSummary wire projection shared by the channel readers (the record -> wire shape construction in crate::state plus the channel serializer, since the member is not in the generated `crate::ops::types::Channel`), not the three handlers, which simply serialize whatever the projection builds. It must emit `failoverSlateConfigurationArn` with the empty string for a channel that has no failover slate.

Cause 1 of 3: BatchGetChannel omits `channels[].failoverSlateConfigurationArn` for every returned channel. Fixture ivs-batch-get-channel-and-stream-key, step task#3. Expected `''` on channels[0] and channels[1]; got None on both.

Cause 2 of 3: BatchGetChannel omits the same member on the second batch read of the same fixture (the mixed real/unknown-ARN batch). Fixture ivs-batch-get-channel-and-stream-key, step task#4. Expected `''` on channels[0]; got None.

Cause 3 of 3: ListChannels omits `channels[].failoverSlateConfigurationArn` on the channel summary. Fixture ivs-channel-lifecycle, step task#3. Expected `''`; got None.

Relation to existing issues: the resolved issue 20261029T172005Z-ivs-channel-and-recording-configuration-reads-omit-documented.md bundled this member together with `multitrackInputConfiguration` and the recording-configuration defaults; those parts now pass (the replay no longer reports them) but `failoverSlateConfigurationArn` is still omitted on all three steps, and a resolved issue covers nothing. The replay reports NO owner for these three clusters. The open issues 20261029T172005Z-ivs-tagging-and-ad-recording-configuration-admission-diverge.md (statuses/admission plus BatchGetChannel's per-item error wording) and 20261029T172005Z-ivs-not-found-error-responses-omit-the-code.md (error envelope) are about different faults on some of the same fixtures and do not carry these keys.

Values excluded as per-run and NOT part of this issue: the same BatchGetChannel clusters also report `ingestEndpoint` / `srt.endpoint` host names (e.g. `d51565af8f48.global-contribute.live-video.net` vs `8e90e7687a9f.global-contribute.live-video.net`) — those hostnames are generated per environment and are not a requirement.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 84 steps, 55 passed, score 65.5%). All three steps replay with the correct status 200 -> 200 and differ only by the missing member:
- BatchGetChannel, ivs-batch-get-channel-and-stream-key#task#3: `$.channels[0].failoverSlateConfigurationArn` expected '' got None; `$.channels[1].failoverSlateConfigurationArn` expected '' got None. (Same cluster also reports `$.channels[0].ingestEndpoint` and `$.channels[0].srt.endpoint` host differences — per-run, excluded.)
- BatchGetChannel, ivs-batch-get-channel-and-stream-key#task#4: `$.channels[0].failoverSlateConfigurationArn` expected '' got None (plus the same excluded host differences).
- ListChannels, ivs-channel-lifecycle#task#3: `$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/a97d402282e0"].failoverSlateConfigurationArn` expected '' got None. The ARN shown is one run's generated channel ARN; the requirement is the member on every summary.
Smithy consulted: shapes com.amazonaws.ivs#Channel, com.amazonaws.ivs#ChannelSummary (neither declares the member -> recorded behavior only), com.amazonaws.ivs#Srt.

## Steps to Reproduce
Boot the emulator from this checkout and talk to the data listener in region us-west-2 (or just run replay_wire_corpus and look at the three clusters below):
1. POST /CreateChannel {"name":"probe"} and note the returned channel ARN.
2. POST /ListChannels {} -> the channel summary carries no `failoverSlateConfigurationArn` member; AWS's recording has it as "" (fixture ivs-channel-lifecycle#task#3).
3. POST /BatchGetChannel {"arns":[<that arn>, <a second created channel arn>]} -> neither returned channel carries `failoverSlateConfigurationArn` (fixture ivs-batch-get-channel-and-stream-key#task#3).
4. POST /BatchGetChannel {"arns":[<that arn>, "arn:aws:ivs:us-west-2:000000000000:channel/does-not-exist"]} -> the one resolved channel again omits the member (fixture ivs-batch-get-channel-and-stream-key#task#4).
Compare each channel object's key set against the recorded response.
