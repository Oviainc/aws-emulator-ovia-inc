Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/607d047704b7"].failoverSlateConfigurationArn
Issue-Commit: 89dcb84bc001d630dd4b5e9068fc0fc72dd2f2e3

# ListChannels omits failoverSlateConfigurationArn from the channel summary it projects

> _Note: this issue was identified against the code base at commit `89dcb84bc001d630dd4b5e9068fc0fc72dd2f2e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
ONE cause.

Cause 1 of 1: the ChannelSummary that ListChannels projects drops the `failoverSlateConfigurationArn` member AWS emits on every projected channel summary, as the empty string when the channel has no failover slate configured. Fixture ivs-channel-lifecycle, step task#3.

AWS: recorded behavior only. Smithy consulted: com.amazonaws.ivs#ListChannels (http POST /ListChannels, code 200 [defaulted], output ListChannelsResponse.channels required -> ChannelList, readonly, paginated nextToken/maxResults) and com.amazonaws.ivs#ChannelSummary, whose declared members are arn / name / latencyMode / authorized / recordingConfigurationArn / tags / insecureIngest / type / preset / playbackRestrictionPolicyArn / adConfigurationArn. This model revision declares NO failover member at all, so the model says nothing about it and this is recorded behavior only: the recorded 200 response nevertheless carries `failoverSlateConfigurationArn` on the channel summary with the value "" for a channel created without a failover slate -- exactly the way ChannelSummary's other optional ARN members are emitted rather than omitted (`recordingConfigurationArn` is documented 'Default: "" (empty string, recording is disabled)' and `playbackRestrictionPolicyArn` 'Default: "" (empty string, no playback restriction policy is applied)'). The requirement is the KEY's presence on every summary -- the empty string for an unconfigured channel, the slate ARN for a configured one. It is not any particular ARN value.

Emulator: the member is absent from the summary object entirely. The replay reports `$.channels[arn=...].failoverSlateConfigurationArn: missing (expected '', got None)` on a step that is otherwise a clean 200 -> 200 with every other projected member matching. The channel ARN inside the JSONPath selector is one run's generated channel ARN and is only what located the summary; it is not part of the requirement.

Why: a client that decides 'is a failover slate configured on this channel?' by comparing `failoverSlateConfigurationArn` against "" reads a missing key / null instead, so a strongly-typed SDK deserializer or a direct string comparison throws, and permissive client code silently reads the channel's failover state as unknown rather than as 'not configured'. Code that lists channels and round-trips a summary into an update drops the member outright.

Fix: the ChannelSummary wire projection shared by the channel listers -- the state record -> wire shape construction plus the channel-summary serializer, since the member is not present in the generated types -- not the ListChannels handler, which serializes whatever the projection builds. It must emit `failoverSlateConfigurationArn` with the empty string for a channel that has no failover slate.

Relation to existing issues: two open issues describe this same fault in prose but the replay reports NO owner for this cluster (owned_by []), so neither one gates it. 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md names it as its 'Cause 3 of 3', but its ListChannels replay key is recorded truncated as `ivs|ListChannels|2xx->2xx|$.channels[arn="arn`; 20261106T174248Z-ivs-listchannels-channel-summary-omits-failoverslateconfigurationarn.md was filed to bind this step, but its key embeds one previous run's generated channel ARN (`.../channel/2b7bef0cdb38`) while this run's cluster key carries a different generated ARN (`.../channel/607d047704b7`), so the ownership join fails on every fresh run and the replay again reports the cluster unowned. Note for whoever fixes this: the per-run ARN inside the cluster key is why these pointers do not join -- the fix must be confirmed by re-replaying ivs-channel-lifecycle#task#3 and checking the key is gone, not by matching a stored ARN-bearing key. The two BatchGetChannel clusters for the same missing member ARE owned by 20261102T071721Z and are deliberately not re-filed here. The other open ivs issues touching this fixture (20261029T172005Z-ivs-tagging-and-ad-recording-configuration-admission-diverge.md, 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md, 20261105T110308Z-ivs-error-envelope-requestid-is-a-uuid-instead.md) are different faults and do not carry this key.

Traces: ivs-channel-lifecycle#task#3 (ListChannels).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, 84 steps total, 55 passed, 24 mismatched, 5 blocked, score 65.5%, 16 divergence clusters.

Cluster divergence_key `service=ivs|ListChannels|status=200->200|keys=$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/607d047704b7"].failoverSlateConfigurationArn`
  fixture ivs-channel-lifecycle, step task#3, operation ListChannels, verdict mismatch, affected_count 1, stub_path empty, owned_by [] (NO owning issue).

Expected status 200, actual status 200 -- the status and every other projected member agree; the body differs by exactly one absent key.
Detail, verbatim: `$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/607d047704b7"].failoverSlateConfigurationArn: missing (expected '', got None)`.

Expected: the ListChannels 200 body's channel summary contains the key `failoverSlateConfigurationArn` with the value "" for a channel created without a failover slate.
Actual: the key is absent from the channel summary object (got None), while `recordingConfigurationArn` and `playbackRestrictionPolicyArn` are present as empty strings on the same object.

Excluded as per-run and NOT part of this issue: the channel ARN `arn:aws:ivs:us-west-2:000000000000:channel/607d047704b7` is one run's generated channel id and serves only as the JSONPath selector that located the summary.

Smithy consulted: com.amazonaws.ivs#ListChannels (POST /ListChannels, code 200, output member `channels` required -> ChannelList); com.amazonaws.ivs#ChannelSummary (members arn/name/latencyMode/authorized/recordingConfigurationArn/tags/insecureIngest/type/preset/playbackRestrictionPolicyArn/adConfigurationArn -- no failover member declared, hence recorded behavior only; `recordingConfigurationArn` and `playbackRestrictionPolicyArn` are documented 'Default: ""').

## Steps to Reproduce
Boot the emulator from this checkout and talk to its data listener in region us-west-2 (or just run replay_wire_corpus and read the ivs|ListChannels cluster):
1. POST /CreateChannel with body {"name":"probe-channel"} -- do not supply any failover slate. Note the returned channel ARN.
2. POST /ListChannels with body {}.
3. Inspect the summary in `channels` whose `arn` is the ARN from step 1 and compare its key set with the recorded response for fixture ivs-channel-lifecycle step task#3.
   Expected: the summary contains `"failoverSlateConfigurationArn": ""`.
   Observed: the key is absent entirely, while `recordingConfigurationArn` and `playbackRestrictionPolicyArn` are present as empty strings on the same object.
4. Equivalently: call replay_wire_corpus() and look for the cluster keyed `service=ivs|ListChannels|status=200->200|keys=...failoverSlateConfigurationArn` on fixture ivs-channel-lifecycle, step task#3. The fix is confirmed when that cluster no longer appears -- do not match on the ARN-bearing key text, which changes every run.
