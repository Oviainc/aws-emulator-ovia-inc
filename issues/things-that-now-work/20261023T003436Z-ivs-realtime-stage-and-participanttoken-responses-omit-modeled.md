Issue-Source: trace-sweep-divergence
Issue-Trace: ivs-realtime-disconnect-participant-unknown-id-is-noop-success
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivs-realtime-disconnect-participant-unknown-id-is-noop-success ivs-realtime-stage-endpoints-present-for-all-protocols ivs-realtime-stage-participant-token-attributes-roundtrip ivs-realtime-create-participant-token-empty-capabilities-list-honored
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime|GetStage|2xx->2xx|$.stage.activeSessionId
Issue-Key: ivs-realtime|GetStage|2xx->2xx|$.stage.autoParticipantRecordingConfiguration
Issue-Key: ivs-realtime|GetStage|2xx->2xx|$.stage.endpoints
Issue-Key: ivs-realtime|CreateStage|2xx->2xx|$.stage.autoParticipantRecordingConfiguration
Issue-Key: ivs-realtime|CreateStage|2xx->2xx|$.stage.endpoints
Issue-Key: ivs-realtime|CreateParticipantToken|2xx->2xx|$.participantToken.capabilities
Issue-Key: ivs-realtime|CreateParticipantToken|2xx->2xx|$.participantToken.token
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# IVS RealTime Stage and ParticipantToken responses omit modeled members

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStage

## Description
The emulator's projection of the modeled Stage and ParticipantToken structures does not match what AWS reports: the stage's data-plane endpoints, its individual-participant-recording configuration and its active-session field are left out of every stage response, and the token projection emits a member AWS does not carry. 3 causes, all in the ivs-realtime stage/token projection.

Cause 1 of 3: GetStage omits stage.endpoints, stage.autoParticipantRecordingConfiguration and stage.activeSessionId. Fixture ivs-realtime-disconnect-participant-unknown-id-is-noop-success, step task#3 (cluster also covers ivs-realtime-stage-endpoints-present-for-all-protocols#task#2).
  AWS: Smithy: GetStage output is Stage, whose members include endpoints (StageEndpoints: events/whip/rtmp/rtmps), autoParticipantRecordingConfiguration (AutoParticipantRecordingConfiguration) and activeSessionId (StageSessionId). The StageEndpoints doc says 'Summary information about various endpoints for a stage. We recommend that you cache these values at stage creation'. Recorded behavior: AWS reports all four endpoint members for every stage, reports autoParticipantRecordingConfiguration populated with the documented defaults (storageConfigurationArn '' = no storage configured, mediaTypes AUDIO_VIDEO, recordingReconnectWindowSeconds 0, recordParticipantReplicas true, thumbnailConfiguration recordingMode DISABLED / storage SEQUENTIAL / targetIntervalSeconds 60), and reports activeSessionId as the empty string when no session is active rather than omitting it. Those default values are exactly the Smithy-documented defaults for AutoParticipantRecordingConfiguration's members.
  Emulator: all three members are absent from the GetStage response (status 200 is correct). StageRecord.endpoints is hard-wired to None (services/service-com-amazonaws-ivsrealtime-.../src/state.rs, the endpoints field and StageRecord::to_stage), active_session_id is None and so is dropped, and autoParticipantRecordingConfiguration is echoed only when the create request supplied one.
  Why: endpoints are how a client learns where to publish/subscribe for a stage - a client that reads stage.endpoints.whip (or rtmp/rtmps/events) gets a null-pointer/absent-field path on the emulator and cannot proceed, and code that reads the recording configuration or the active-session field off a stage sees an absent member where AWS always supplies one.
  Fix: the stage projection and its state (StageRecord::to_stage and the StageRecord fields it reads) must always populate endpoints for a stage, project the recording configuration with the modeled defaults when the caller supplied none, and report activeSessionId as the empty value rather than dropping the member. The endpoint host values themselves are environment-specific and are not the requirement - presence of all four members with well-formed scheme-correct values is.
  Traces: ivs-realtime-disconnect-participant-unknown-id-is-noop-success#task#3, ivs-realtime-stage-endpoints-present-for-all-protocols#task#2.

Cause 2 of 3: CreateStage omits stage.autoParticipantRecordingConfiguration and stage.endpoints on the freshly created stage. Fixture ivs-realtime-stage-participant-token-attributes-roundtrip, step task#1.
  AWS: Smithy: CreateStage output member stage is the same Stage shape. Recorded behavior: the create response already carries the endpoints block (events/rtmp/rtmps/whip) and the default autoParticipantRecordingConfiguration described in cause 1 - which is why the model's own StageEndpoints doc tells callers to cache the endpoints 'at stage creation'.
  Emulator: both members are absent from the CreateStage response (status 200 correct). src/ops/create_stage.rs builds StageRecord with endpoints: None and stores whatever the request supplied for the recording configuration without defaulting it.
  Why: a client that follows AWS's documented advice and caches the endpoints returned by CreateStage has nothing to cache, and must then discover the stage is unusable later.
  Fix: same component as cause 1 - assign the stage's endpoints at creation in the CreateStage handler/state and project the recording-configuration defaults, so CreateStage and GetStage report one identical Stage.
  Traces: ivs-realtime-stage-participant-token-attributes-roundtrip#task#1.

Cause 3 of 3: CreateParticipantToken emits participantToken.capabilities where AWS does not carry it, when the request supplied an empty capabilities list. Fixture ivs-realtime-create-participant-token-empty-capabilities-list-honored, step task#2.
  AWS: Smithy: CreateParticipantTokenRequest.capabilities -> ParticipantTokenCapabilities @length 0..2, so an empty list is a legal request and the documented PUBLISH,SUBSCRIBE default must NOT be substituted for it. Recorded behavior only: with an explicitly empty capabilities list, the recorded participantToken does not carry a capabilities member at all (the replay verdict on that key is unexpected_present), i.e. AWS honours the empty list by reporting no capabilities rather than an empty array.
  Emulator: the token projection always emits capabilities (src/state.rs, ParticipantTokenRecord::to_participant_token sets capabilities: Some(self.capabilities.clone())), so an empty request list comes back as capabilities: [].
  Why: a client that tests for the presence of capabilities to decide whether a token is restricted sees a different shape than AWS gives it; round-tripping a token record through the emulator changes the member set of the response.
  Fix: the ParticipantToken projection in the service's state module - omit capabilities when the issued token holds none, the same way attributes is already omitted when empty.
  Traces: ivs-realtime-create-participant-token-empty-capabilities-list-honored#task#2.

Not filed from this cluster: the same step also reports participantToken.token as a value_mismatch against a shaped-not-asserted expectation. A participant token is a secret minted per run, so its recorded value is not a requirement and that assertion is excluded from this issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline.

Cause 1 - fixture ivs-realtime-disconnect-participant-unknown-id-is-noop-success step task#3, operation GetStage, verdict mismatch, expected 200 / actual 200 (cluster size 2, also ivs-realtime-stage-endpoints-present-for-all-protocols#task#2):
  $.stage.activeSessionId: missing (expected '', got None)
  $.stage.autoParticipantRecordingConfiguration: missing (expected {'hlsConfiguration': {'targetSegmentDurationSeconds': 6}, 'mediaTypes': ['AUDIO_VIDEO'], 'recordParticipantReplicas': True, 'recordingReconnectWindowSeconds': 0, 'storageConfigurationArn': '', 'thumbnailConfiguration': {'recordingMode': 'DISABLED', 'storage': ['SEQUENTIAL'], 'targetIntervalSeconds': 60}}, got None)
  $.stage.endpoints: missing (expected {'events': 'wss://global.events.live-video.net', 'rtmp': ..., 'rtmps': ..., 'whip': ...}, got None)

Cause 2 - fixture ivs-realtime-stage-participant-token-attributes-roundtrip step task#1, operation CreateStage, verdict mismatch, expected 200 / actual 200:
  $.stage.autoParticipantRecordingConfiguration: missing (expected the same default block as above, got None)
  $.stage.endpoints: missing (expected {'events': 'wss://global.events.live-video.net', 'rtmp': 'rtmp://<host>.global-contribute.live-video.net/app/', 'rtmps': 'rtmps://<host>.global-contribute.live-video.net:443/app/', 'whip': 'https://<host>.global-bm.whip.live-video.net'}, got None) - hosts are environment-specific examples, the member's presence is the requirement

Cause 3 - fixture ivs-realtime-create-participant-token-empty-capabilities-list-honored step task#2, operation CreateParticipantToken, verdict mismatch, expected 200 / actual 200:
  $.participantToken.capabilities: unexpected_present (emulator emits the member; the recording carries none)
  (also reported on this step and deliberately NOT filed: $.participantToken.token value_mismatch against a shaped-not-asserted expectation - a per-run secret)

Code evidence: services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/state.rs line 107 (`pub endpoints: Option<StageEndpoints>` documented as deliberately left None), lines 121-134 (StageRecord::to_stage), lines 162-172 (ParticipantTokenRecord::to_participant_token, capabilities always Some); src/ops/create_stage.rs line 252 (`endpoints: None`).
Model evidence: smithy_query(shape="Stage,StageEndpoints,AutoParticipantRecordingConfiguration,ParticipantToken") on the service's smithy model.

## Steps to Reproduce
1. Boot the emulator from mainline and replay trace set simparity-wire-aws-v1.
2. Cause 1: POST /CreateStage, then POST /GetStage with the returned ARN. Observe the returned stage has no endpoints, no autoParticipantRecordingConfiguration and no activeSessionId member; AWS returns all four endpoint members, the default recording configuration and an empty activeSessionId. Fixtures ivs-realtime-disconnect-participant-unknown-id-is-noop-success step task#3 and ivs-realtime-stage-endpoints-present-for-all-protocols step task#2.
3. Cause 2: POST /CreateStage and inspect the create response itself - endpoints and autoParticipantRecordingConfiguration are already absent there. Fixture ivs-realtime-stage-participant-token-attributes-roundtrip, step task#1.
4. Cause 3: POST /CreateParticipantToken with "capabilities": [] for an existing stage. Observe the response's participantToken carries "capabilities": []; the recording carries no capabilities member. Fixture ivs-realtime-create-participant-token-empty-capabilities-list-honored, step task#2.
