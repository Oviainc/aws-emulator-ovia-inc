Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/ecf9b6952343"].failoverSlateConfigurationArn
Issue-Commit: 655f0a7c26e2eb42d468d60ed812ed49ea4e0b40

# ListChannels channel summary omits failoverSlateConfigurationArn empty-string default

> _Note: this issue was identified against the code base at commit `655f0a7c26e2eb42d468d60ed812ed49ea4e0b40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
ONE net-new cause. The ChannelSummary objects IVS ListChannels returns are missing the `failoverSlateConfigurationArn` member that the recorded AWS responses carry on every projected channel.

Cause 1 of 1: ListChannels omits `channels[].failoverSlateConfigurationArn` on the channel summary. Fixture ivs-channel-lifecycle, step task#3 (status 200 -> 200, the only differing key).

AWS: recorded behavior only. Smithy consulted: shape com.amazonaws.ivs#ChannelSummary declares arn/name/latencyMode/authorized/recordingConfigurationArn/tags/insecureIngest/type/preset/playbackRestrictionPolicyArn/adConfigurationArn, and com.amazonaws.ivs#Channel adds ingestEndpoint/playbackUrl/srt/multitrackInputConfiguration/containerFormat — NEITHER shape declares a failover member in this model revision, so the model says nothing about it. The recording is the authority: the recorded 200 ListChannels response carries `failoverSlateConfigurationArn` on each entry of `channels`, with the value `""` (empty string) for a channel that has no failover slate configured. That is the same emit-the-empty-string convention the model documents for the sibling ARN members on the same shape (`recordingConfigurationArn`: 'Default: "" (empty string, recording is disabled)', `playbackRestrictionPolicyArn`: 'Default: "" ... no playback restriction policy is applied'). The requirement is the KEY's PRESENCE with the empty string on an unconfigured channel; a channel with a slate configured would carry that slate's ARN. No status or error code is involved — the status matches at 200.

Emulator: the key is absent from every channel summary the emulator serves. Replay reports `$.channels[arn="..."].failoverSlateConfigurationArn: missing (expected '', got None)` for the created channel. The ARN in the replay key is one run's generated channel ARN and is NOT part of the requirement; the requirement applies to every summary the operation returns.

Why: a client that decides whether a failover slate is configured by comparing `failoverSlateConfigurationArn` against the empty string reads a missing key / null instead — strongly typed SDK consumers see None and treat the state as unknown, and dynamic clients comparing to "" throw or branch wrongly. Code that round-trips a ListChannels summary back into an update silently drops the member.

Fix: the ChannelSummary wire projection used by the ListChannels reader — the record -> wire-shape construction for channel summaries in the ivs crate's state/serializer layer, not the ListChannels handler, which serializes whatever the projection builds. The member is not in the generated types, so the projection must add it and populate it with the empty string when the channel has no failover slate.

Traces: ivs-channel-lifecycle#task#3 (ListChannels).

Relation to existing issues: the open issue 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md describes the same family of omission and names this ListChannels step in its prose, but the replay reports NO owner for this cluster — that issue's ownership key is `ivs|ListChannels|2xx->2xx|$.channels[arn="arn` , which is pinned to a per-run channel ARN and therefore does not match this run's cluster, so the step is not gated by it. The BatchGetChannel clusters in the same replay ARE owned by that issue and are not re-filed here. The resolved issue 20261029T172005Z-ivs-channel-and-recording-configuration-reads-omit-documented.md also touched this member; a resolved issue covers nothing.

Excluded as per-run and NOT part of this issue: generated channel ARNs, ingest/SRT hostnames and SRT passphrases, and request ids — these differ in every environment and are not requirements.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 84 steps total, 55 passed, 24 mismatched, score 65.5%).

Divergence cluster: service=ivs | ListChannels | status=200->200 | fixture ivs-channel-lifecycle, step task#3, affected_count 1, owned_by [] (unowned), stub_path empty.

Expected (recorded AWS response, 200): each entry of `channels` carries `failoverSlateConfigurationArn` with value '' for the newly created, slate-less channel.
Actual (emulator, 200): `$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/ecf9b6952343"].failoverSlateConfigurationArn: missing (expected '', got None)`. The ARN shown is this run's generated value.

All other asserted members of the summary matched, and no status, error code or error message diverged for this step.

Smithy consulted: shapes com.amazonaws.ivs#ChannelSummary and com.amazonaws.ivs#Channel (full member lists above; neither declares the member -> recorded behavior only), including the documented 'Default: ""' wording on recordingConfigurationArn and playbackRestrictionPolicyArn.

## Steps to Reproduce
Boot the emulator from this checkout and talk to the IVS data listener in region us-west-2 (or run replay_wire_corpus and read the ivs-channel-lifecycle#task#3 cluster).
1. POST /CreateChannel with body {"name":"probe-channel"} and note the returned channel ARN. Do not configure any failover slate.
2. POST /ListChannels with body {} .
3. Inspect the summary object for that ARN in the `channels` array: the emulator's object has no `failoverSlateConfigurationArn` key at all.
4. Compare its key set against the recorded response for fixture ivs-channel-lifecycle step task#3, which contains `failoverSlateConfigurationArn: ""` on that summary. The status is 200 in both cases; only the key set differs.
