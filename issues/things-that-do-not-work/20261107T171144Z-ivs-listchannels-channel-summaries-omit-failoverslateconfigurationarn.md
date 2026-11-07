Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/5d7af6f7bebc"].failoverSlateConfigurationArn
Issue-Commit: 5430332628414c7579166edd2caebd2a2c1b2de2

# IVS ListChannels channel summaries omit failoverSlateConfigurationArn

> _Note: this issue was identified against the code base at commit `5430332628414c7579166edd2caebd2a2c1b2de2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListChannels

## Description
ONE cause.

Cause 1 of 1: ListChannels omits the `failoverSlateConfigurationArn` member from every `ChannelSummary` it projects. Fixture ivs-channel-lifecycle, step task#3 (status 200 -> 200; the only divergence on the step is the missing member).

AWS: recorded behavior only. Smithy consulted: com.amazonaws.ivs#ChannelSummary and com.amazonaws.ivs#Channel in services/service-com-amazonaws-ivs-amazoninteractivevideoservice/smithy-model/com-amazonaws-ivs-amazoninteractivevideoservice.json — in this model revision ChannelSummary declares arn/name/latencyMode/authorized/recordingConfigurationArn/tags/insecureIngest/type/preset/playbackRestrictionPolicyArn/adConfigurationArn and NO failover member, so the model says nothing about it. The recording shows AWS emitting `failoverSlateConfigurationArn` as a PRESENT key on every channel summary, carrying the empty string `""` when the channel has no failover slate configured — the same way the model's other "Default: \"\"" ARN members (recordingConfigurationArn, playbackRestrictionPolicyArn, adConfigurationArn) are emitted as "" rather than omitted. The requirement is the key's presence with "" for an unconfigured channel; a channel that has a slate would carry that slate's ARN. No error code or message is involved: the step is a successful 200 on both sides.

Emulator: the member is absent from the summary object the emulator serves — replay reports `$.channels[<the created channel>].failoverSlateConfigurationArn: missing (expected '', got None)`. The emulator's ListChannels response otherwise matches the recording.

Why: a client that decides "does this channel have a failover slate?" by reading `failoverSlateConfigurationArn` and comparing it to the empty string gets a missing key / null instead, so the comparison raises (typed SDKs) or silently reads as "unknown" (untyped clients); code that lists channels and round-trips a summary into an update or into a diff drops the member entirely, so the field it never saw cannot be preserved.

Fix: the ChannelSummary wire projection used by the ListChannels reader — the record -> wire shape construction in the ivs crate's state plus the channel-summary serializer (the member is not in the generated summary type, so it must be added to the projection, not just echoed), not the handler, which serializes whatever the projection builds. It must emit `failoverSlateConfigurationArn` with the empty string for a channel with no failover slate.

Relation to existing issues: the open issue 20261102T071721Z-ivs-channel-projections-still-omit-failoverslateconfigurationarn-on-reads.md describes this same member on BatchGetChannel and names a ListChannels cause, but its ListChannels replay key is recorded truncated (`Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn`) and keyed on one run's generated channel ARN, so the replay reports NO owner for this cluster — it is unowned and refiled here with a usable fixture/step binding. A fixer closing the BatchGetChannel issue should close this one in the same projection change; it will not be exercised by that issue's replay pointer.

Values excluded as per-run and NOT part of this issue: the generated channel ARN that appears in the JSONPath of the recorded expectation (arn:aws:ivs:us-west-2:000000000000:channel/5d7af6f7bebc) is one run's id; the requirement is the member on every summary, not that ARN.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true, 84 steps, 55 passed, 24 mismatched, score 65.5%).

Cluster: service=ivs|ListChannels|status=200->200|keys=$.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/5d7af6f7bebc"].failoverSlateConfigurationArn
  fixture_id: ivs-channel-lifecycle
  step_key:   task#3
  verdict:    mismatch
  expected_status 200, actual_status 200
  detail: $.channels[arn="arn:aws:ivs:us-west-2:000000000000:channel/5d7af6f7bebc"].failoverSlateConfigurationArn: missing (expected '', got None)
  owned_by: [] (no open issue carries this replay key), stub_path: "" (operation is implemented)

Expected (recorded AWS): the channel summary object contains the key `failoverSlateConfigurationArn` with value `""` for the channel created earlier in the fixture.
Actual (emulator): the key is absent from the summary object; every other asserted member of the summary matches.

Smithy consulted: com.amazonaws.ivs#ChannelSummary, com.amazonaws.ivs#Channel (neither declares the member -> recorded behavior only).

## Steps to Reproduce
Boot the emulator from this checkout and talk to its data listener in region us-west-2 (or run replay_wire_corpus and read the ivs-channel-lifecycle#task#3 cluster).
1. POST /CreateChannel with {"name":"probe"} — no failover slate configured. Note the returned channel ARN.
2. POST /ListChannels with {} .
3. Inspect the summary for that ARN in `channels`: the emulator's object has no `failoverSlateConfigurationArn` key. The recorded AWS response for the same step carries `"failoverSlateConfigurationArn": ""`.
4. Compare the summary's key set against the recorded response for fixture ivs-channel-lifecycle step task#3.
