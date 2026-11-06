Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/2b7bef0cdb38"].failoverSlateConfigurationArn
Issue-Commit: 85bf1a921c90e8ec0578295db955b3f20c9762fc

# IVS ListChannels channel summary omits failoverSlateConfigurationArn

> _Note: this issue was identified against the code base at commit `85bf1a921c90e8ec0578295db955b3f20c9762fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
ONE cause.

Cause 1 of 1: the channel summary ListChannels projects drops the `failoverSlateConfigurationArn` member that AWS emits on every projected channel, as the empty string when the channel has no failover slate configured. Fixture ivs-channel-lifecycle, step task#3.

AWS: recorded behavior only. Smithy consulted: com.amazonaws.ivs#ListChannels (POST /ListChannels, output ListChannelsResponse.channels -> ChannelList, status 200) and com.amazonaws.ivs#ChannelSummary, whose members are arn/name/latencyMode/authorized/recordingConfigurationArn/tags/insecureIngest/type/preset/playbackRestrictionPolicyArn/adConfigurationArn — the model revision in this checkout declares NO failover member, so the model says nothing about it. The recorded 200 response nevertheless carries `failoverSlateConfigurationArn` on the channel summary with the value `""` for a channel created without a failover slate — the same way ChannelSummary's other "Default: \"\"" ARN members (`recordingConfigurationArn`, `playbackRestrictionPolicyArn`) are emitted as the empty string rather than omitted. The requirement is the KEY's presence on every summary, with the empty string for an unconfigured channel (and the slate ARN for a configured one); it is not any particular ARN value.

Emulator: the member is absent from the summary object entirely — the replay reports `$.channels[arn="..."].failoverSlateConfigurationArn` expected '' got None, on a step that is otherwise a clean 200 -> 200 with every other member matching. The ARN in the JSONPath selector is one run's generated channel ARN and is not part of the requirement.

Why: a client that decides "is a failover slate configured on this channel?" by comparing `failoverSlateConfigurationArn` against `""` reads a missing key / null instead, so a strongly-typed SDK deserializer or a direct string comparison throws, and permissive client code silently reads the channel's failover state as unknown rather than as "not configured". Code that lists channels and round-trips a summary into an update drops the member outright.

Fix: the ChannelSummary wire projection shared by the channel listers (the state record -> wire shape construction plus the channel summary serializer; the member is not present in the generated types, so it belongs in that projection), not the ListChannels handler, which serializes whatever the projection builds. It must emit `failoverSlateConfigurationArn` with the empty string for a channel that has no failover slate.

Relation to existing issues: open issue 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md describes this same fault in prose as its "Cause 3 of 3", but its replay pointer does NOT carry this cluster's key — its ListChannels Issue-Key is recorded truncated as `ivs|ListChannels|2xx->2xx|$.channels[arn="arn`, and the replay accordingly reports NO owner for this cluster while it does report that issue as owner of the two BatchGetChannel clusters. This issue exists to bind the ListChannels step to a replayable key so the fix is actually gated on it; a fixer closing either one should re-replay ivs-channel-lifecycle#task#3. The other open ivs issues on this fixture (20261029T172005Z-ivs-tagging-and-ad-recording-configuration-admission-diverge.md, 20261103T113752Z-ivs-createchannel-srt-block-mints-an-endpoint-and.md, 20261105T110308Z-ivs-error-envelope-requestid-is-a-uuid-instead.md) are different faults and do not carry this key.

Traces: ivs-channel-lifecycle#task#3 (ListChannels).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 84 steps, 55 passed, score 65.5%, 16 divergence clusters).

Cluster service=ivs|ListChannels|status=200->200|keys=$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/2b7bef0cdb38"].failoverSlateConfigurationArn
  fixture ivs-channel-lifecycle, step task#3, verdict mismatch, owned_by [] (no owning issue), stub_path empty, affected_count 1.
  Expected status 200, actual status 200 — the status and every other projected member agree.
  Detail: `$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/2b7bef0cdb38"].failoverSlateConfigurationArn: missing (expected '', got None)`.

Expected: the ListChannels 200 body's channel summary contains the key `failoverSlateConfigurationArn` with value `""` for a channel created without a failover slate.
Actual: the key is absent from the channel summary object (got None).

The channel ARN `arn:aws:ivs:us-west-2:000000000000:channel/2b7bef0cdb38` is the generated id of one run's channel and is only the selector that located the summary; it is not part of the requirement.

Smithy consulted: com.amazonaws.ivs#ListChannels (http POST /ListChannels, code 200, output member `channels` required, target ChannelList), com.amazonaws.ivs#ChannelSummary (member list above; no failover member declared -> recorded behavior only; `recordingConfigurationArn` and `playbackRestrictionPolicyArn` are documented "Default: \"\" (empty string...)").

## Steps to Reproduce
Boot the emulator from this checkout and talk to its data listener in region us-west-2 (or run replay_wire_corpus and read the ivs|ListChannels cluster):
1. POST /CreateChannel with body {"name":"probe-channel"} — do not supply any failover slate. Note the returned channel ARN.
2. POST /ListChannels with body {} .
3. Inspect the summary in `channels` whose `arn` is the ARN from step 1 and compare its key set with the recorded response for fixture ivs-channel-lifecycle step task#3.
   Expected: the summary contains `"failoverSlateConfigurationArn": ""`.
   Observed: the key is absent entirely, while `recordingConfigurationArn` and `playbackRestrictionPolicyArn` are present as empty strings on the same object.
4. Equivalently: replay_wire_corpus() and look for the cluster keyed service=ivs|ListChannels|status=200->200|keys=...failoverSlateConfigurationArn on fixture ivs-channel-lifecycle, step task#3.
