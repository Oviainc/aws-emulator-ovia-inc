Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-channel-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-channel-lifecycle ivs-viewer-session-revocation ivs-batch-get-channel-and-stream-key ivs-recording-configuration-rendition-selection-persists ivs-recording-configuration-thumbnail-configuration-persists
Issue-Shape-Id: com.amazonaws.ivs#AmazonInteractiveVideoService
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.multitrackInputConfiguration
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.endpoint
Issue-Key: ivs|CreateChannel|2xx->2xx|$.channel.srt.passphrase
Issue-Key: ivs|GetChannel|2xx->2xx|$.channel.multitrackInputConfiguration
Issue-Key: ivs|ListChannels|2xx->2xx|$.channels[arn="arn
Issue-Key: ivs|UpdateChannel|2xx->2xx|$.channel.multitrackInputConfiguration
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].failoverSlateConfigurationArn
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].ingestEndpoint
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].multitrackInputConfiguration
Issue-Key: ivs|BatchGetChannel|2xx->2xx|$.channels[*].srt.endpoint
Issue-Key: ivs|CreateRecordingConfiguration|2xx->2xx|$.recordingConfiguration.thumbnailConfiguration
Issue-Key: ivs|GetRecordingConfiguration|2xx->2xx|$.recordingConfiguration.thumbnailConfiguration
Issue-Key: ivs|CreateRecordingConfiguration|2xx->2xx|$.recordingConfiguration.renditionConfiguration
Issue-Key: ivs|GetRecordingConfiguration|2xx->2xx|$.recordingConfiguration.renditionConfiguration
Issue-Commit: 4fccfcb9e83e9e12ca223d9fc117bac8a0390f59

# IVS channel and recording-configuration reads omit documented default sub-structures (10 causes)

> _Note: this issue was identified against the code base at commit `4fccfcb9e83e9e12ca223d9fc117bac8a0390f59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetChannel

## Description
Every IVS response that projects a Channel or a RecordingConfiguration drops the sub-structures AWS materialises with their documented defaults when the request did not name them: Channel.multitrackInputConfiguration, the channel summaries' failoverSlateConfigurationArn, RecordingConfiguration.thumbnailConfiguration and RecordingConfiguration.renditionConfiguration. TEN causes, one per failing fixture step; each must be closed.

Shared AWS behavior. Smithy (com.amazonaws.ivs): Channel$multitrackInputConfiguration targets MultitrackInputConfiguration{enabled default false, policy (MultitrackPolicy), maximumResolution (MultitrackMaximumResolution)}; RecordingConfiguration$thumbnailConfiguration targets ThumbnailConfiguration{recordingMode doc "Default: INTERVAL", targetIntervalSeconds doc "Default: 60", storage doc "Default: SEQUENTIAL"}; RecordingConfiguration$renditionConfiguration targets RenditionConfiguration{renditionSelection doc "Default: ALL"}. The recorded responses show AWS materialising these defaults rather than omitting the member: a channel created without multitrack input reports multitrackInputConfiguration {enabled false, policy ALLOW, maximumResolution FULL_HD}; a recording configuration created with only a thumbnailConfiguration still reports renditionConfiguration {renditionSelection ALL}, and one created with only a renditionConfiguration still reports thumbnailConfiguration {recordingMode INTERVAL, storage [SEQUENTIAL], targetIntervalSeconds 60}. failoverSlateConfigurationArn is not in this model revision at all — recorded behavior only: AWS emits it on channel summaries and on BatchGetChannel's channels as the empty string when no failover slate is configured, the same way Channel's other "Default: \"\"" ARN members are emitted.

Shared emulator behavior: the member is absent (`got None`) from every projection — create, read, update, list and batch read alike — so a client sees no multitrack configuration, no failover slate member and only whichever of the two recording sub-configurations the create request happened to carry.

Why: a client reading channel.multitrackInputConfiguration.enabled, or recordingConfiguration.thumbnailConfiguration.targetIntervalSeconds, gets a missing-key error or a null where AWS always gives it a value; code that round-trips a Get into an Update drops the configuration entirely, and anything deciding "is a failover slate set?" by comparing the member to "" cannot.

Fix: the Channel and RecordingConfiguration projections in crate::state (the record -> wire shape construction the handlers share), which must materialise these documented defaults at create time rather than echoing only what the request supplied.

Note on values excluded as per-run: the same clusters also report ingestEndpoint / srt.endpoint host names and srt.passphrase differing from the recording. Those are generated per environment and are NOT part of this issue.

Cause 1 of 10: CreateChannel response omits channel.multitrackInputConfiguration. Fixture ivs-channel-lifecycle, step task#1.
Cause 2 of 10: GetChannel omits channel.multitrackInputConfiguration. Fixture ivs-channel-lifecycle, step task#2 (cluster also covers ivs-channel-lifecycle#task#5 and ivs-viewer-session-revocation#task#5).
Cause 3 of 10: ListChannels omits channels[].failoverSlateConfigurationArn (expected ""). Fixture ivs-channel-lifecycle, step task#3.
Cause 4 of 10: UpdateChannel omits channel.multitrackInputConfiguration. Fixture ivs-channel-lifecycle, step task#4.
Cause 5 of 10: CreateChannel omits channel.multitrackInputConfiguration on the second fixture that exercises it. Fixture ivs-viewer-session-revocation, step setup#1.
Cause 6 of 10: BatchGetChannel omits channels[].failoverSlateConfigurationArn AND channels[].multitrackInputConfiguration for every returned channel. Fixture ivs-batch-get-channel-and-stream-key, step task#3.
Cause 7 of 10: CreateRecordingConfiguration with a renditionConfiguration omits the default thumbnailConfiguration. Fixture ivs-recording-configuration-rendition-selection-persists, step task#2.
Cause 8 of 10: GetRecordingConfiguration for that configuration omits the default thumbnailConfiguration. Fixture ivs-recording-configuration-rendition-selection-persists, step task#3.
Cause 9 of 10: CreateRecordingConfiguration with a thumbnailConfiguration omits the default renditionConfiguration. Fixture ivs-recording-configuration-thumbnail-configuration-persists, step task#2.
Cause 10 of 10: GetRecordingConfiguration for that configuration omits the default renditionConfiguration. Fixture ivs-recording-configuration-thumbnail-configuration-persists, step task#3.

No open IVS issue covers these projections (the open behavioral issue 20261023T004057Z-ivs-7-behavioural-divergences-error-messages-dropped-on.md is about error wording, stubs, IAM, S3 and the data plane).

## Evidence
All ten steps replayed with status 200 -> 200 and only missing members:
- CreateChannel ivs-channel-lifecycle#task#1: $.channel.multitrackInputConfiguration expected {'enabled': False, 'maximumResolution': 'FULL_HD', 'policy': 'ALLOW'} got None. (srt.endpoint / srt.passphrase differences in the same cluster are per-run and excluded.)
- GetChannel ivs-channel-lifecycle#task#2: $.channel.multitrackInputConfiguration expected {'enabled': False, 'maximumResolution': 'FULL_HD', 'policy': 'ALLOW'} got None; cluster covers ivs-channel-lifecycle#task#5 and ivs-viewer-session-revocation#task#5.
- UpdateChannel ivs-channel-lifecycle#task#4: same member missing.
- CreateChannel ivs-viewer-session-revocation#setup#1: same member missing.
- ListChannels ivs-channel-lifecycle#task#3: $.channels[arn=...].failoverSlateConfigurationArn expected '' got None.
- BatchGetChannel ivs-batch-get-channel-and-stream-key#task#3: $.channels[0].failoverSlateConfigurationArn expected '' got None; $.channels[0].multitrackInputConfiguration expected {'enabled': False, 'maximumResolution': 'FULL_HD', 'policy': 'ALLOW'} got None; same two for $.channels[1].
- CreateRecordingConfiguration ivs-recording-configuration-rendition-selection-persists#task#2 and GetRecordingConfiguration #task#3: $.recordingConfiguration.thumbnailConfiguration expected {'recordingMode': 'INTERVAL', 'storage': ['SEQUENTIAL'], 'targetIntervalSeconds': 60} got None.
- CreateRecordingConfiguration ivs-recording-configuration-thumbnail-configuration-persists#task#2 and GetRecordingConfiguration #task#3: $.recordingConfiguration.renditionConfiguration expected {'renditionSelection': 'ALL'} got None.

## Steps to Reproduce
Boot the emulator from this checkout, region us-west-2:
1. POST /CreateChannel {"name":"probe"} -> response channel has no multitrackInputConfiguration; POST /GetChannel and /UpdateChannel for the same ARN -> same omission.
2. POST /ListChannels -> the summary has no failoverSlateConfigurationArn member; POST /BatchGetChannel {"arns":[<that arn>]} -> the channel has neither failoverSlateConfigurationArn nor multitrackInputConfiguration.
3. POST /CreateRecordingConfiguration with a destinationConfiguration and ONLY renditionConfiguration {"renditionSelection":"ALL"} -> response has no thumbnailConfiguration; GetRecordingConfiguration repeats the omission.
4. POST /CreateRecordingConfiguration with ONLY thumbnailConfiguration {"recordingMode":"INTERVAL","targetIntervalSeconds":60,"storage":["SEQUENTIAL"]} -> response has no renditionConfiguration; GetRecordingConfiguration repeats the omission.
