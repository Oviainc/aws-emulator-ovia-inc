Issue-Source: trace-sweep-divergence
Issue-Trace: groundstation-config-lifecycle-tracking-and-downlink
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: groundstation-config-lifecycle-tracking-and-downlink groundstation-config-other-capability-types groundstation-mission-profile-lifecycle
Issue-Shape-Id: com.amazonaws.groundstation#GroundStation
Issue-Key: groundstation|GetConfig|4xx->2xx|$.message
Issue-Key: groundstation|GetConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: groundstation|GetConfig|4xx->2xx|status
Issue-Key: groundstation|GetConfig|2xx->2xx|$.tags
Issue-Key: groundstation|GetConfig|2xx->2xx|$.configData.trackingConfig.autotrack
Issue-Key: groundstation|GetConfig|2xx->2xx|$.name
Issue-Key: groundstation|GetMissionProfile|2xx->2xx|$.tags
Issue-Key: groundstation|GetMissionProfile|2xx->2xx|$.minimumViableContactDurationSeconds
Issue-Key: groundstation|GetMissionProfile|2xx->2xx|$.name
Issue-Key: groundstation|GetMissionProfile|4xx->2xx|$.message
Issue-Key: groundstation|GetMissionProfile|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: groundstation|GetMissionProfile|4xx->2xx|status
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# Ground Station resource state: deletes and updates not persisted, tags omitted

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfig

## Description
The Ground Station config and mission-profile stores do not apply lifecycle writes: a delete leaves the resource readable, an update leaves the prior attribute values in place, and the `tags` member is never serialized on a read. Six causes, all in the same state/serializer layer.

Cause 1 of 6: DeleteConfig does not remove the config, so a following GetConfig still succeeds.
AWS: Smithy GetConfig -> GET /config/{configType}/{configId}, errors DependencyException(531), InvalidParameterException(431), ResourceNotFoundException(434). DeleteConfig is DELETE on the same URI and is @idempotent. After a successful DeleteConfig the recording shows GetConfig answering HTTP 434 with x-amzn-errortype: ResourceNotFoundException (message example: 'No config found with id 502a164a-6a9c-4758-8d8b-bbdf536254d9 and type tracking').
Emulator: GetConfig answers HTTP 200 with the deleted config's body; no `message` and no x-amzn-errortype header.
Why: a client cannot tell a deleted config from a live one. Cleanup/teardown logic and "does this exist?" probes that rely on ResourceNotFoundException never fire, and callers keep ARNs to configs AWS would already have removed.
Fix: DeleteConfig's state mutation (the config store's remove path) and GetConfig's not-found branch in the operation handler.

Cause 2 of 6: DeleteMissionProfile does not remove the mission profile, so a following GetMissionProfile still succeeds.
AWS: Smithy GetMissionProfile -> GET /missionprofile/{missionProfileId}, errors include ResourceNotFoundException(434); DeleteMissionProfile is DELETE on the same URI, @idempotent. Recording: after the delete, GetMissionProfile answers 434 with x-amzn-errortype: ResourceNotFoundException (message example: 'MissionProfile not found').
Emulator: HTTP 200 with the deleted profile's body, no `message`, no x-amzn-errortype.
Why: same as cause 1 for mission profiles — deletion is unobservable.
Fix: DeleteMissionProfile's state mutation and GetMissionProfile's not-found branch.

Cause 3 of 6: UpdateConfig is a no-op — neither `name` nor `configData` is persisted.
AWS: Smithy UpdateConfig -> PUT /config/{configType}/{configId} with required `name` (SafeName, @length 1..256, @pattern ^[ a-zA-Z0-9_:-]{1,256}$) and required `configData` (ConfigTypeData). A subsequent GetConfig must return the updated values.
Emulator: after an UpdateConfig that renamed the tracking config and set configData.trackingConfig.autotrack, GetConfig returns the pre-update `name` and autotrack 'PREFERRED' instead of the requested 'REQUIRED'.
Why: a client that updates a config and re-reads it sees stale state, so idempotent reconcile loops never converge and a config AWS would have reconfigured keeps serving the old capability settings.
Fix: UpdateConfig's handler/state write (it must replace name and configData on the stored config).

Cause 4 of 6: UpdateMissionProfile is a no-op — neither `name` nor `minimumViableContactDurationSeconds` is persisted.
AWS: Smithy UpdateMissionProfile -> PUT /missionprofile/{missionProfileId} with optional `name` (SafeName) and `minimumViableContactDurationSeconds` (PositiveDurationInSeconds, @range 1..21600). GetMissionProfile returns both members.
Emulator: after an UpdateMissionProfile setting a new name and minimumViableContactDurationSeconds=45, GetMissionProfile returns the pre-update name and 30.
Why: same as cause 3 — mission-profile edits are silently dropped, and contact scheduling then uses a duration the caller already changed.
Fix: UpdateMissionProfile's handler/state write (apply every present member onto the stored profile).

Cause 5 of 6: GetConfig omits the `tags` member entirely.
AWS: Smithy GetConfigResponse has member `tags` targeting TagsMap (map<String,String>). The recording shows `tags` present and serialized as an empty object `{}` for an untagged config.
Emulator: `tags` is absent from the JSON body (got None).
Why: an SDK/consumer that reads response.tags gets null rather than an empty map, so tag-diffing code must special-case the emulator; anything iterating tags without a null guard breaks.
Fix: GetConfig's response serializer (emit `tags`, defaulting to an empty map rather than skipping the member).

Cause 6 of 6: GetMissionProfile omits the `tags` member entirely.
AWS: Smithy GetMissionProfileResponse has member `tags` targeting TagsMap. Recording shows `tags`: {} for an untagged profile.
Emulator: `tags` absent from the body (got None).
Why: as cause 5, for mission profiles.
Fix: GetMissionProfile's response serializer.

Note: the delete-not-honored cause is reported by replay as one cluster of 7 affected steps spanning GetConfig teardowns in groundstation-config-lifecycle-tracking-and-downlink, groundstation-config-other-capability-types and groundstation-mission-profile-lifecycle, so fixing it should clear all of them. state_reset_failures was 0 for this replay, so these are not cross-fixture state bleed.

Traces (one representative per cause):
- Cause 1: groundstation-config-lifecycle-tracking-and-downlink#teardown#9 (GetConfig, expected 434, got 200)
- Cause 2: groundstation-mission-profile-lifecycle#teardown#13 (GetMissionProfile, expected 434, got 200)
- Cause 3: groundstation-config-lifecycle-tracking-and-downlink#task#6 (GetConfig after UpdateConfig)
- Cause 4: groundstation-mission-profile-lifecycle#task#8 (GetMissionProfile after UpdateMissionProfile)
- Cause 5: groundstation-config-lifecycle-tracking-and-downlink#task#3 (GetConfig, $.tags missing)
- Cause 6: groundstation-mission-profile-lifecycle#task#5 (GetMissionProfile, $.tags missing)

## Evidence
Cause 1 — groundstation-config-lifecycle-tracking-and-downlink#teardown#9, GetConfig:
  expected status 434, actual 200
  $.message: missing (expected 'No config found with id 502a164a-6a9c-4758-8d8b-bbdf536254d9 and type tracking', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)
  cluster of 7 affected steps: groundstation-config-lifecycle-tracking-and-downlink#teardown#9, #teardown#11, groundstation-config-other-capability-types#task#5, #task#11, groundstation-mission-profile-lifecycle#teardown#14, #teardown#15, #teardown#16

Cause 2 — groundstation-mission-profile-lifecycle#teardown#13, GetMissionProfile:
  expected status 434, actual 200
  $.message: missing (expected 'MissionProfile not found', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException', got None)

Cause 3 — groundstation-config-lifecycle-tracking-and-downlink#task#6, GetConfig (200 vs 200):
  $.name: expected 'sim2real-probe-groundstation-gs-tracking-cfg-renamed-00f2068c', got 'sim2real-probe-groundstation-gs-tracking-cfg-00f2068c'
  $.configData.trackingConfig.autotrack: expected 'REQUIRED', got 'PREFERRED'

Cause 4 — groundstation-mission-profile-lifecycle#task#8, GetMissionProfile (200 vs 200):
  $.name: expected 'sim2real-probe-groundstation-mission-profile-renamed-225a704e', got 'sim2real-probe-groundstation-mission-profile-225a704e'
  $.minimumViableContactDurationSeconds: expected 45, got 30

Cause 5 — groundstation-config-lifecycle-tracking-and-downlink#task#3, GetConfig (200 vs 200):
  $.tags: missing (expected {}, got None); cluster also covers groundstation-config-other-capability-types#task#3 and #task#9

Cause 6 — groundstation-mission-profile-lifecycle#task#5, GetMissionProfile (200 vs 200):
  $.tags: missing (expected {}, got None)

Smithy citations: GetConfigResponse.tags -> TagsMap; GetMissionProfileResponse.tags -> TagsMap; GetConfig/GetMissionProfile errors include ResourceNotFoundException @httpError 434; UpdateConfig requires name+configData; UpdateMissionProfile.minimumViableContactDurationSeconds @range 1..21600.

## Steps to Reproduce
1. Replay the wire corpus: replay_wire_corpus().
2. Cause 1: fixture groundstation-config-lifecycle-tracking-and-downlink, step teardown#9 — CreateConfig (tracking), DeleteConfig on it, then GET /config/tracking/{configId}. Expect 434 + x-amzn-errortype: ResourceNotFoundException; emulator returns 200 with the config body.
3. Cause 2: fixture groundstation-mission-profile-lifecycle, step teardown#13 — CreateMissionProfile, DeleteMissionProfile, then GET /missionprofile/{id}. Expect 434 ResourceNotFoundException; emulator returns 200.
4. Cause 3: fixture groundstation-config-lifecycle-tracking-and-downlink, step task#6 — PUT /config/tracking/{configId} with a new name and configData.trackingConfig.autotrack=REQUIRED, then GET the same config. Emulator returns the pre-update name and autotrack=PREFERRED.
5. Cause 4: fixture groundstation-mission-profile-lifecycle, step task#8 — PUT /missionprofile/{id} with a new name and minimumViableContactDurationSeconds=45, then GET it. Emulator returns the old name and 30.
6. Causes 5 and 6: fixture groundstation-config-lifecycle-tracking-and-downlink step task#3 and fixture groundstation-mission-profile-lifecycle step task#5 — GET an untagged config / mission profile and inspect the body for the `tags` key. AWS serializes `tags`: {}; the emulator omits the member.
