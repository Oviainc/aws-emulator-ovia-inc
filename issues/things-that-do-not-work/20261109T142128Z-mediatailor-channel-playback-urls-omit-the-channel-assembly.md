Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-start-stop-and-log-configuration
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-start-stop-and-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|DescribeChannel|2xx->2xx|$.Outputs[*].DualStackPlaybackUrl
Issue-Key: mediatailor|DescribeChannel|2xx->2xx|$.Outputs[*].PlaybackUrl
Issue-Commit: 0c7f0017b5788ebb0b2bf7aa1a6523722bad5325

# MediaTailor channel playback URLs omit the channel-assembly endpoint labels

> _Note: this issue was identified against the code base at commit `0c7f0017b5788ebb0b2bf7aa1a6523722bad5325`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
Cause 1 of 1: the per-output playback URLs a channel reports (ResponseOutputItem.PlaybackUrl and ResponseOutputItem.DualStackPlaybackUrl) are built on the service's own regional API host instead of the MediaTailor channel-assembly playback host, so the host has the wrong DNS structure.

AWS: Smithy model services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json, DescribeChannel -> DescribeChannelResponse.Outputs (ResponseOutputs of ResponseOutputItem); ResponseOutputItem.PlaybackUrl (__string, required) is documented as "The URL that your player uses for playback" and ResponseOutputItem.DualStackPlaybackUrl as "The dual-stack (IPv4 and IPv6) URL that your player uses for playback". The model does not state the URL grammar, so for the host form this is recorded behavior only: the recording shows a channel-assembly playback host of the form https://<32-hex-channel-label>.<channel-subdomain-label>.channel-assembly.mediatailor.<region>.amazonaws.com for PlaybackUrl and the same host under the dual-stack suffix .<region>.api.aws for DualStackPlaybackUrl, with the path /v1/channel/<ChannelName>/<ManifestName>.m3u8. The fixed text of the host is the literal label sequence "channel-assembly.mediatailor.<region>" plus the partition DNS suffix (amazonaws.com, or api.aws for dual stack); the variable parts are the generated per-channel label(s), the region, and the ChannelName/ManifestName in the path. The concrete label values in the recording (e22f8cdd439c8fce5bcc4b6039df4f27, p05sqb) are per-run generated values and are NOT part of the requirement.

Emulator: it serves the two URLs on a host with the channel-assembly labels missing entirely - https://<28-hex-label>.mediatailor.<region>.amazonaws.com/v1/channel/<ChannelName>/<ManifestName>.m3u8 and https://<28-hex-label>.mediatailor.<region>.api.aws/... . Replay reports $.Outputs[0].PlaybackUrl and $.Outputs[0].DualStackPlaybackUrl as host_not_own with status 200 on both sides: expected host e22f8cdd439c8fce5bcc4b6039df4f27.p05sqb.channel-assembly.mediatailor.us-west-2.amazonaws.com, actual dca413efef606f0c87878817ffb5.mediatailor.us-west-2.amazonaws.com (and the .api.aws pair likewise). The announced host is neither AWS's channel-assembly playback host nor the host the emulator is actually reached on.

Why: a client that takes PlaybackUrl/DualStackPlaybackUrl from CreateChannel/DescribeChannel and hands it to a player, or that asserts/derives the channel-assembly domain (for example to pin a CDN origin, a DNS/cert check, or a regional routing rule), gets a hostname in the service's control-plane API domain rather than the channel-assembly playback domain. The URL does not resolve to anything serving the manifest, and any code that parses the host to recover the channel-assembly subdomain structure fails.

Fix: the playback-URL derivation in the channel handler that mints these values - playback_urls() / playback_host_label() in the CreateChannel handler, whose stored ResponseOutputItem values are what DescribeChannel (and the other channel reads) echo back. It must emit the channel-assembly host label sequence for both the single-stack and dual-stack forms, and the reads must report the stored outputs unchanged.

Traces: mediatailor-channel-start-stop-and-log-configuration#task#4 (DescribeChannel, representative) and mediatailor-channel-start-stop-and-log-configuration#task#6 (the same cluster, second channel read).

## Evidence
Replay cluster service=mediatailor|DescribeChannel|status=200->200|keys=$.Outputs[0].DualStackPlaybackUrl,$.Outputs[0].PlaybackUrl (verdict mismatch, affected_count 2).

Expected (recorded AWS) vs actual (emulator), status 200 on both sides:
  $.Outputs[0].PlaybackUrl
    expected: https://e22f8cdd439c8fce5bcc4b6039df4f27.p05sqb.channel-assembly.mediatailor.us-west-2.amazonaws.com/v1/channel/sim2real-probe-mediatailor-ch-6517e4a6/default.m3u8
    actual:   https://dca413efef606f0c87878817ffb5.mediatailor.us-west-2.amazonaws.com/v1/channel/sim2real-probe-mediatailor-ch-6517e4a6/default.m3u8
  $.Outputs[0].DualStackPlaybackUrl
    expected: https://e22f8cdd439c8fce5bcc4b6039df4f27.p05sqb.channel-assembly.mediatailor.us-west-2.api.aws/v1/channel/sim2real-probe-mediatailor-ch-6517e4a6/default.m3u8
    actual:   https://dca413efef606f0c87878817ffb5.mediatailor.us-west-2.api.aws/v1/channel/sim2real-probe-mediatailor-ch-6517e4a6/default.m3u8
  comparator verdict on both keys: host_not_own

The path (/v1/channel/<ChannelName>/<ManifestName>.m3u8) matches; the divergence is confined to the host's label structure - the "channel-assembly" label (and the per-channel subdomain label preceding it) are absent, so the URL sits in the control-plane API domain mediatailor.<region>.amazonaws.com. Emulator source: services/service-com-amazonaws-mediatailor-mediatailor/src/ops/create_channel.rs lines 165-183 format the hosts as "https://{label}.mediatailor.{region}.amazonaws.com{path}" and "https://{label}.mediatailor.{region}.api.aws{path}"; lines 148-152 mint the 28-hex label. Generated label values and region are per-environment and are not themselves the finding.

No open issue covers this: the open MediaTailor issues concern error shapes (20261030T045613Z-mediatailor-error-responses-omit-message-and-requestid-and.md, 20261103T024708Z-mediatailor-startchannel-miss-answers-400-badrequestexception-instead-of.md, 20261104T201408Z-mediatailor-configurelogsforchannel-channel-miss-answers-400-badrequestexception-instead.md, 20261030T005241Z-mediatailor-createchannel-duplicate-name-refusal-uses-a-non.md), tagging (20261030T045613Z-mediatailor-tagging-path-broken-valid-tagresource-refused-tags.md), the DeleteChannel stub, and CloudFormation resource types.

## Steps to Reproduce
1. Replay fixture mediatailor-channel-start-stop-and-log-configuration against the emulator.
2. Step task#4 issues GET /channel/sim2real-probe-mediatailor-ch-6517e4a6 (DescribeChannel) for the channel created earlier in the fixture.
3. Inspect $.Outputs[0].PlaybackUrl and $.Outputs[0].DualStackPlaybackUrl in the 200 response: the host is <label>.mediatailor.us-west-2.amazonaws.com / <label>.mediatailor.us-west-2.api.aws, with no ".channel-assembly." label sequence, where the recording has <label>.<subdomain>.channel-assembly.mediatailor.us-west-2.amazonaws.com / ....us-west-2.api.aws.
4. Step task#6 (DescribeChannel again in the same fixture) shows the same two keys diverging identically.
5. The same host is minted at creation time, so a CreateChannel response in the same fixture carries it too.
