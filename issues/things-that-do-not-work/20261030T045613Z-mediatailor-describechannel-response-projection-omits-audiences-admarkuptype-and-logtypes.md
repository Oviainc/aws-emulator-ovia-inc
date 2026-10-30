Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-start-stop-and-log-configuration
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-start-stop-and-log-configuration
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|DescribeChannel|2xx->2xx|$.Audiences
Issue-Key: mediatailor|DescribeChannel|2xx->2xx|$.ChannelState
Issue-Key: mediatailor|DescribeChannel|2xx->2xx|$.LogConfiguration.LogTypes[*]
Issue-Key: mediatailor|DescribeChannel|2xx->2xx|$.Outputs[*].HlsPlaylistSettings.AdMarkupType
Issue-Commit: 841ccf5bd961485229918da026bcdc910fdfcb94

# DescribeChannel response projection omits Audiences, AdMarkupType and LogTypes and reports stale ChannelState

> _Note: this issue was identified against the code base at commit `841ccf5bd961485229918da026bcdc910fdfcb94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeChannel

## Description
Two root causes (Cause 1 of 2, Cause 2 of 2). Both are the same defect family: the channel read projects fewer members than AWS does, and one of them is also stale. Replay fixture mediatailor-channel-start-stop-and-log-configuration.

Cause 1 of 2 - DescribeChannel after StartChannel/ConfigureLogsForChannel under-reports four members (fixture mediatailor-channel-start-stop-and-log-configuration, step task#4).

AWS: Smithy model services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json - DescribeChannel (GET /channel/{ChannelName}, success 200, errors: []) returns ChannelState (enum ChannelState, values RUNNING | STOPPED), LogConfiguration (LogConfigurationForChannel, @required, member LogTypes -> list of LogType, values AS_RUN), Audiences (list Audiences of String) and Outputs (ResponseOutputs of ResponseOutputItem, whose HlsPlaylistSettings carries AdMarkupType -> list of AdMarkupType, values DATERANGE | SCTE35_ENHANCED). The model declares no defaults for AdMarkupType or Audiences, so for those two this is recorded behavior only: the recording shows AWS reporting Audiences as a present-but-EMPTY list for a channel created with no audiences, and HlsPlaylistSettings.AdMarkupType as ["DATERANGE"] for an output whose request omitted it - i.e. both are always-present members, one defaulted. After StartChannel succeeds AWS reports ChannelState = RUNNING; after ConfigureLogsForChannel enables a log type AWS reports LogConfiguration.LogTypes = ["AS_RUN"].

Emulator: HTTP 200 (status matches) but the body drops every one of those: $.ChannelState is "STOPPED" where AWS reports "RUNNING"; $.Outputs[0].HlsPlaylistSettings.AdMarkupType is absent where AWS reports a one-element list containing DATERANGE; $.LogConfiguration.LogTypes[0] is absent (the list is empty) where AWS reports AS_RUN; $.Audiences is absent entirely where AWS reports an empty list. The projection is services/service-com-amazonaws-mediatailor-mediatailor/src/state.rs ChannelRecord::describe_output / ChannelRecord::create_output (lines 421-474): audiences is passed through as Option and stays None for a channel created without any, and log_types/channel_state are only ever moved by operations that currently answer 501. AdMarkupType is cloned verbatim from the request in services/service-com-amazonaws-mediatailor-mediatailor/src/ops/create_channel.rs lines 347-362, so an omitted AdMarkupType is never defaulted. The stale ChannelState and empty LogTypes are downstream of the mediatailor StartChannel scaffold stub, which open issue "Implement mediatailor StartChannel: unimplemented scaffold stub" owns - that issue covers making StartChannel answer, it does NOT cover this read's projection, which must still report the moved state and the always-present members once the mutators work.

Why: a client that polls DescribeChannel until ChannelState == RUNNING never advances, because the emulator reports STOPPED forever. A client that reads Audiences or Outputs[].HlsPlaylistSettings.AdMarkupType dereferences an absent member instead of a list and faults or silently takes an empty-config branch; the same for LogConfiguration.LogTypes, which the model marks @required, so a strict SDK deserializer sees a required structure with no log types where AWS reported one.

Fix: the DescribeChannel read's response projection - ChannelRecord::describe_output/create_output in src/state.rs - must always emit Audiences as a list (empty when the channel has none) and must emit LogConfiguration.LogTypes as the stored list; the CreateChannel handler's output-building must default an output's HlsPlaylistSettings.AdMarkupType to the single value AWS reports when the request omits it, and persist it so the read reports it too. ChannelState must be whatever the start/stop mutators last stored, so it has to be re-checked once StartChannel is implemented.

Traces: mediatailor-channel-start-stop-and-log-configuration#task#4.

Cause 2 of 2 - the same read, after the stop leg, still omits AdMarkupType, LogTypes and Audiences (fixture mediatailor-channel-start-stop-and-log-configuration, step task#6).

AWS: same model facts as Cause 1. On this read ChannelState agrees (both report STOPPED), which isolates the projection defect from the state-machine one: AWS still reports Outputs[0].HlsPlaylistSettings.AdMarkupType = ["DATERANGE"], LogConfiguration.LogTypes = ["AS_RUN"] and Audiences = [] (recorded behavior for the first and third, model-declared members for all three).

Emulator: HTTP 200 with $.Outputs[0].HlsPlaylistSettings.AdMarkupType, $.LogConfiguration.LogTypes[0] and $.Audiences all absent - identical omissions to Cause 1, proving they are not a side effect of the channel being started.

Why: the channel's advertised ad-markup type and enabled log types are invisible to a client regardless of channel state, so configuration a caller just wrote cannot be read back, and audience-aware clients see no audience member at all.

Fix: same component as Cause 1 - the channel read's projection in src/state.rs plus the AdMarkupType default on the create path. This cause must be closed independently of the StartChannel stub.

Traces: mediatailor-channel-start-stop-and-log-configuration#task#6.

## Evidence
Fixture: mediatailor-channel-start-stop-and-log-configuration (trace set simparity-wire-aws-v1)

Cause 1 - step task#4, operation DescribeChannel, verdict mismatch, expected status 200, actual status 200:
  $.ChannelState: value_mismatch (expected 'RUNNING', got 'STOPPED')
  $.Outputs[0].HlsPlaylistSettings.AdMarkupType: missing (expected ['DATERANGE'], got None)
  $.LogConfiguration.LogTypes[0]: missing (expected 'AS_RUN', got None)
  $.Audiences: missing (expected [], got None)

Cause 2 - step task#6, operation DescribeChannel, verdict mismatch, expected status 200, actual status 200:
  $.Outputs[0].HlsPlaylistSettings.AdMarkupType: missing (expected ['DATERANGE'], got None)
  $.LogConfiguration.LogTypes[0]: missing (expected 'AS_RUN', got None)
  $.Audiences: missing (expected [], got None)

Model evidence (smithy_query query="operation" operation="DescribeChannel", query="shape" shape="LogConfigurationForChannel,ChannelState,ResponseOutputItem,HlsPlaylistSettings,LogType,AdMarkupType,Audiences"):
  DescribeChannel: GET /channel/{ChannelName}, code 200, errors []
  DescribeChannelResponse.LogConfiguration -> LogConfigurationForChannel, REQUIRED; LogConfigurationForChannel.LogTypes -> list of LogType (enum: AS_RUN)
  DescribeChannelResponse.Audiences -> Audiences (list of String)
  DescribeChannelResponse.ChannelState -> ChannelState (enum: RUNNING, STOPPED)
  ResponseOutputItem.HlsPlaylistSettings -> HlsPlaylistSettings; HlsPlaylistSettings.AdMarkupType -> list of AdMarkupType (enum: DATERANGE, SCTE35_ENHANCED)

Code citations for the diverging projection:
  services/service-com-amazonaws-mediatailor-mediatailor/src/state.rs:421-474 (create_output/describe_output: audiences forwarded as Option, log_types forwarded as stored)
  services/service-com-amazonaws-mediatailor-mediatailor/src/state.rs:393-409 (ChannelRecord.channel_state, ChannelRecord.log_types)
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/create_channel.rs:347-362 (hls_playlist_settings cloned verbatim from the request; AdMarkupType never defaulted)

Related open issue (does NOT cover these steps; the replay reported them unowned): "Implement mediatailor StartChannel: unimplemented scaffold stub" (20261030T005241Z-mediatailor-startchannel-stub.md).

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Create a channel with one HLS output whose HlsPlaylistSettings omit AdMarkupType and whose request carries no Audiences:
   aws mediatailor create-channel --channel-name probe-ch \
     --outputs '[{"ManifestName":"default","SourceGroup":"default","HlsPlaylistSettings":{"ManifestWindowSeconds":30}}]' \
     --playback-mode LOOP --tier BASIC
3. Enable channel logging:  aws mediatailor configure-logs-for-channel --channel-name probe-ch --log-types AS_RUN
4. Start the channel:        aws mediatailor start-channel --channel-name probe-ch
5. aws mediatailor describe-channel --channel-name probe-ch
   -> ChannelState must be RUNNING (emulator: STOPPED)
   -> LogConfiguration.LogTypes must be ["AS_RUN"] (emulator: empty)
   -> Audiences must be present as [] (emulator: member absent)
   -> Outputs[0].HlsPlaylistSettings.AdMarkupType must be ["DATERANGE"] (emulator: member absent)
6. Stop the channel and describe again: ChannelState becomes STOPPED on both, but AdMarkupType, LogTypes and Audiences are still omitted by the emulator.
7. Re-replay with replay_wire_corpus: fixture mediatailor-channel-start-stop-and-log-configuration steps task#4 and task#6 must both pass.
