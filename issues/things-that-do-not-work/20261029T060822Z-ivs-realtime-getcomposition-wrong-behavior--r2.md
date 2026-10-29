Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 6cada3bea20bb0dfad4860907220259726a5f31e

# A composition destination's recording and thumbnail configuration is echoed back raw, with none of the documented defaults this crate materialises for every other configuration it stores

_Filed: 2026-10-29T06:08:22Z_
_Operation: GetComposition | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `6cada3bea20bb0dfad4860907220259726a5f31e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartComposition` persists each `DestinationConfiguration` exactly as the request sent it, and `GetComposition` reports that stored value unchanged. So for an S3 destination the members of `S3DestinationConfiguration.recordingConfiguration.hlsConfiguration` and of `S3DestinationConfiguration.thumbnailConfigurations[0]` are reported ABSENT whenever the starting request left them out, although each of them documents a default:

* `CompositionRecordingHlsConfiguration$targetSegmentDurationSeconds` — "Default: 2."
* `CompositionThumbnailConfiguration$targetIntervalSeconds` — "Default: 60."
* `CompositionThumbnailConfiguration$storage` — "Default: `SEQUENTIAL`."

This contradicts the rule this crate states and applies everywhere else it stores a configuration. The SAME handler applies "the model's documented default for every member it left out" to the composition's `layout`; `CreateEncoderConfiguration` applies it to `video`; and `CreateStage`/`UpdateStage` apply it to the stage's `autoParticipantRecordingConfiguration` — including the two structurally identical sub-objects, `ParticipantRecordingHlsConfiguration$targetSegmentDurationSeconds` (via `crate::state::effective_hls_configuration`) and `ParticipantThumbnailConfiguration` (via `crate::state::effective_thumbnail_configuration`, which materialises `targetIntervalSeconds`, `storage` and `recordingMode` even when the request supplied no thumbnail configuration at all).

The result is that one crate answers two ways for the same question: a stage always reports a fully-defaulted recording/thumbnail configuration, while a composition destination reports whatever the caller typed and nothing more. A client that reads `targetSegmentDurationSeconds` off a composition destination finds no member, and cannot tell "absent" from the documented 2.

There is no `effective_destination`/`effective_recording_configuration`-style seam for the destination side at all: `DestinationRecord.configuration` is the request object, and `DestinationRecord::to_destination` clones it.

## Evidence

Model — `smithy_query(query="shape", shape="CompositionRecordingHlsConfiguration")`, member `targetSegmentDurationSeconds`: "Defines the target duration for recorded segments generated when using composite recording. **Default: 2.**"

Model — `smithy_query(query="shape", shape="CompositionThumbnailConfiguration")`, member `targetIntervalSeconds`: "The targeted thumbnail-generation interval in seconds. **Default: 60.**"; member `storage`: "… You can enable both `SEQUENTIAL` and `LATEST`. **Default: `SEQUENTIAL`.**"

The rule the crate states for the sibling member of the SAME request — `src/ops/start_composition.rs:287-290`:
```
        // The layout the composition will run with: what the request asked for, with the
        // model's documented default for every member it left out, applied in the one
        // place this service applies them (`crate::state::effective_layout`).
        let layout = crate::state::effective_layout(input.layout);
```

The destination side of the same handler, which applies no default at all — `src/ops/start_composition.rs:418-442`:
```
        let destinations: Vec<crate::state::DestinationRecord> = input
            .destinations
            .into_iter()
            .map(|configuration| {
                let id = crate::state::mint_id(...);
                ...
                crate::state::DestinationRecord {
                    id,
                    state: crate::state::DESTINATION_STATE_STARTING.to_string(),
                    started_at: now,
                    ended_at: None,
                    configuration,
                    detail,
                }
            })
            .collect();
```

What `GetComposition` then reports — `src/state.rs:1347-1356`:
```
    pub fn to_destination(&self) -> Destination {
        Destination {
            id: self.id.clone(),
            state: self.state.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
            configuration: self.configuration.clone(),
            detail: self.detail.clone(),
        }
    }
```
reached from `src/ops/get_composition.rs:167-169`:
```
        Ok(GetCompositionOutput {
            composition: Some(record.to_composition()),
        })
```

The same crate materialising the identical kind of default for the stage-side counterparts — `src/state.rs:172-184`:
```
/// The HLS configuration a stage reports, with the one member the caller left out filled
/// in from the model's documented default for it.
pub fn effective_hls_configuration(
    requested: Option<ParticipantRecordingHlsConfiguration>,
) -> ParticipantRecordingHlsConfiguration {
    ParticipantRecordingHlsConfiguration {
        target_segment_duration_seconds: Some(
            requested
                .and_then(|configuration| configuration.target_segment_duration_seconds)
                .unwrap_or(DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS),
        ),
    }
}
```
and `src/state.rs:189-198`:
```
pub fn default_recording_configuration() -> AutoParticipantRecordingConfiguration {
    AutoParticipantRecordingConfiguration {
        storage_configuration_arn: NO_STORAGE_CONFIGURATION_ARN.to_string(),
        media_types: Some(default_recording_media_types()),
        thumbnail_configuration: Some(effective_thumbnail_configuration(None)),
        recording_reconnect_window_seconds: Some(DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS),
        hls_configuration: Some(effective_hls_configuration(None)),
        record_participant_replicas: Some(DEFAULT_RECORD_PARTICIPANT_REPLICAS),
    }
}
```

Test coverage: `tests/all/compositions.rs:481-486` pins the pure echo, with no reason tied to the members' documented defaults:
```
        for (index, (asked, answered)) in requested.iter().zip(reported.iter()).enumerate() {
            assert_eq!(
                &answered["configuration"], asked,
                "case {case:?}: destination {index} must report the configuration it was \
                 created from, got {answered}"
            );
```
and every case it feeds (`tests/all/compositions.rs:363-393`) either omits `recordingConfiguration`/`thumbnailConfigurations` entirely or supplies every member, so no case exercises a partially-supplied destination configuration. The stage-side defaults are pinned by `tests/all/documented_member_defaults.rs` and `tests/all/stages.rs:1251-1263`.

Note: this is NOT the gap in the open issue `20261028T173549Z-ivs-realtime-startcomposition-wrong-behavior--r3`, which is about the three `GridConfiguration`/`PipConfiguration` members whose documented default is the empty string inside `effective_layout`. These members live on `S3DestinationConfiguration`, have non-empty documented defaults, and have no defaulting seam at all.

## Steps to Reproduce

1. CreateStorageConfiguration and CreateEncoderConfiguration; note both ARNs.
2. CreateStage; note the stage ARN.
3. StartComposition with stageArn=<stage> and destinations=[{"s3":{"storageConfigurationArn":<storage>,"encoderConfigurationArns":[<encoder>],"recordingConfiguration":{"format":"HLS"}}}] — i.e. supply `recordingConfiguration` but omit `hlsConfiguration`, and omit `thumbnailConfigurations` entirely.
4. Read `composition.destinations[0].configuration.s3.recordingConfiguration` in the response: it carries only `format`. There is no `hlsConfiguration.targetSegmentDurationSeconds`, although the member documents `Default: 2`; and there is no `thumbnailConfigurations`, although its members document `Default: 60` / `Default: SEQUENTIAL`.
5. GetComposition on the returned ARN: the same un-defaulted configuration comes back, so the omission is persisted and not a response-shaping accident.
6. Contrast with CreateStage/GetStage on the same engine: a stage whose request supplied `autoParticipantRecordingConfiguration.storageConfigurationArn=<storage>` and nothing else reports a full `thumbnailConfiguration` (recordingMode, targetIntervalSeconds, storage) and a full `hlsConfiguration.targetSegmentDurationSeconds`.
