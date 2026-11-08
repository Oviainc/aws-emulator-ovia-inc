Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/933a248a9abb"].failoverSlateConfigurationArn
Issue-Commit: 71abee76eb2f160c739b94d3274b51074cc31505

# ListChannels channel summaries still drop the failoverSlateConfigurationArn empty-string member

> _Note: this issue was identified against the code base at commit `71abee76eb2f160c739b94d3274b51074cc31505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
ONE net-new cause. This is the only divergence cluster in the simparity-wire-aws-v1 replay that the report leaves UNOWNED (owned_by [], stub_path empty); the other 15 ivs clusters are owned by open issues or blocked by a filed stub and are not re-filed here.

Cause 1 of 1: the ChannelSummary objects ivs ListChannels projects omit the `failoverSlateConfigurationArn` member that the recorded AWS response carries on every summary. Fixture ivs-channel-lifecycle, step task#3 (status 200 -> 200; the member is the ONLY differing key in the cluster).

AWS: recorded behavior only. Smithy consulted (services/service-com-amazonaws-ivs-amazoninteractivevideoservice/smithy-model/com-amazonaws-ivs-amazoninteractivevideoservice.json): ListChannels is POST /ListChannels, http code 200, output ListChannelsResponse with required member `channels` -> ChannelList, plus `nextToken`; errors AccessDeniedException(403)/ConflictException(409)/ValidationException(400). The ChannelSummary shape in THIS model revision does not declare a failover member at all, so the model says nothing about it and the recording is the authority: the recorded 200 response carries `failoverSlateConfigurationArn` on each entry of `channels`, valued as the empty string ("") for a channel with no failover slate configured. That matches the emit-the-empty-string convention the model documents for the sibling ARN members on the same summary (recordingConfigurationArn and playbackRestrictionPolicyArn both document 'Default: ""'). The requirement is the KEY'S PRESENCE on every projected summary, with the empty string when no slate is configured and the configured slate's ARN otherwise. No status, error code or error message is involved: both sides answer 200.

Emulator: the key is absent from every channel summary served by ListChannels. Replay reports `$.channels[arn="..."].failoverSlateConfigurationArn: missing (expected '', got None)`. The channel ARN appearing in the replay key is one run's generated value and is NOT part of the requirement — the requirement holds for every summary the operation returns, in any environment.

Why: a client that decides whether a failover slate is configured by comparing `failoverSlateConfigurationArn` to "" instead sees an absent key / null. Strongly typed SDK consumers read None and treat the failover state as unknown rather than unconfigured; dynamic clients comparing against "" raise or take the wrong branch; code that round-trips a ListChannels summary into a subsequent update silently drops the member.

Fix: the ChannelSummary wire projection the ListChannels reader consumes — the record-to-wire-shape construction for channel summaries in the ivs crate's state/serializer layer, not the ListChannels handler, which faithfully serializes whatever the projection builds. Because the member is absent from the generated summary type, the projection must carry it and default it to the empty string. Applying the fix at the projection also covers the BatchGetChannel/GetChannel readers that share it.

Relation to existing issues (none of them gates this step): four open issues describe this same omission in prose — 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md, 20261106T174248Z-ivs-listchannels-channel-summary-omits-failoverslateconfigurationarn.md, 20261107T100912Z-ivs-listchannels-omits-failoverslateconfigurationarn-from-the-channel-summary-it.md, 20261107T171144Z-ivs-listchannels-channel-summaries-omit-failoverslateconfigurationarn.md and 20261107T175056Z-ivs-listchannels-channel-summary-omits-failoverslateconfigurationarn-empty-string-default.md. The replay nonetheless reports NO owner for this cluster, because each of those issues' ownership key embeds a per-run channel ARN (e.g. `ivs|ListChannels|2xx->2xx|$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/ecf9b6952343"].failoverSlateConfigurationArn`), which cannot match a later run's freshly generated ARN. That key-pinning defect is why the cluster re-surfaces as net-new every sweep; a fixer should treat all of these as ONE defect and close them together. The BatchGetChannel clusters in this same replay (ivs-batch-get-channel-and-stream-key#task#3 and #task#4) ARE owned by 20261102T071721Z and are deliberately NOT re-filed here.

Excluded as per-run and NOT requirements: generated channel ARNs, ingest/SRT hostnames, SRT passphrases, and request ids.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, 84 steps total, 55 passed, 24 mismatched, 5 blocked, 7 not_implemented, score 65.5%, 16 divergence clusters.

Divergence cluster: divergence_key `service=ivs|ListChannels|status=200->200|keys=$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/933a248a9abb"].failoverSlateConfigurationArn`; fixture ivs-channel-lifecycle, step task#3; verdict mismatch; affected_count 1; owned_by [] (UNOWNED); stub_path empty.

Expected (recorded AWS, status 200): every entry of `channels` carries `failoverSlateConfigurationArn`, valued "" for the newly created channel that has no failover slate.
Actual (emulator, status 200): `$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/933a248a9abb"].failoverSlateConfigurationArn: missing (expected '', got None)` — the key is absent. The ARN shown is this run's generated value and is not asserted as a requirement.

No status, error code or error message diverged for this step; every other asserted member of the summary matched.

Smithy consulted: ListChannels operation signature (POST /ListChannels, 200, output ListChannelsResponse{channels required, nextToken}, errors AccessDenied/Conflict/Validation) and the ChannelSummary shape, which does not declare the member in this revision -> recorded behavior only.

## Steps to Reproduce
Boot the emulator from this checkout and talk to the ivs data listener in us-west-2 (or run replay_wire_corpus and read the ivs-channel-lifecycle#task#3 cluster).
1. POST /CreateChannel with body {"name":"probe-channel"}; note the returned channel ARN. Configure no failover slate.
2. POST /ListChannels with body {}.
3. Inspect the summary object for that ARN inside the `channels` array: the emulator's object has no `failoverSlateConfigurationArn` key at all.
4. Compare the key set against the recorded response for fixture ivs-channel-lifecycle step task#3, whose matching summary carries `failoverSlateConfigurationArn: ""`. Both responses are status 200; only the key set differs.
5. Repeat after creating a second channel to confirm the omission is on every projected summary, not one record.
