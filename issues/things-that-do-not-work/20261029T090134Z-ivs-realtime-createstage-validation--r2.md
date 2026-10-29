Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# A thumbnail targetIntervalSeconds is accepted and reported while recordingMode is not INTERVAL, the one mode the model says it is configurable in

_Filed: 2026-10-29T09:01:34Z_
_Operation: CreateStage | Class: validation_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AutoParticipantRecordingConfiguration.thumbnailConfiguration.targetIntervalSeconds` is documented as configurable ONLY when the sibling `recordingMode` is `INTERVAL`. The crate's one place that resolves a stage's recording configuration — `crate::state::effective_thumbnail_configuration`, shared by `CreateStage` and `UpdateStage` — copies whatever interval the request sent without ever looking at `recordingMode`, and `recordingMode` itself defaults to `DISABLED`. So a request that configures a thumbnail interval while the configuration is in (or defaults to) `DISABLED` mode is answered 200, the interval is persisted on the stage, and `GetStage`/`ListStages`/`UpdateStage` report it back — a configured value in a mode the model states it cannot be configured in.

This is the same shape of rule the crate already enforces one file over: `CreateIngestConfiguration` refuses the documented-but-unmodelled pairing between `ingestProtocol` and `insecureIngest` with the operation's modelled `ValidationException`, and says so explicitly ("It is a documented requirement the model carries no trait for — neither member constrains the other — so the handler enforces it"). The thumbnail pairing is carried by no trait either (`targetIntervalSeconds` has only `@range 1..86400`), and no handler, helper or comment in the crate enforces or even mentions it, so nothing refuses the combination and nothing normalises it away.

The same seam is reached by `UpdateStage` (`update_stage.rs:176-192` -> `resolved_recording_configuration` -> `effective_recording_configuration` -> `effective_thumbnail_configuration`), so both writers of a stage's recording configuration accept it.

Note for the reviewer: one row of an existing default-filling test currently pins the acceptance (see evidence); that test's stated purpose is the default-filling rule, not this pairing, and the row happens to send an interval with no `recordingMode`.

## Evidence

Model (`smithy_query(query="shape", shape="ParticipantThumbnailConfiguration")`), member `targetIntervalSeconds`:
  "<p>The targeted thumbnail-generation interval in seconds. This is configurable only if <code>recordingMode</code> is <code>INTERVAL</code>. Default: 60.</p>"
and member `recordingMode`: "<p>Thumbnail recording mode. Default: <code>DISABLED</code>.</p>" (only values `INTERVAL`, `DISABLED`).

Code that ignores the pairing — services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/state.rs:153-170:
```
pub fn effective_thumbnail_configuration(
    requested: Option<ParticipantThumbnailConfiguration>,
) -> ParticipantThumbnailConfiguration {
    let requested = requested.unwrap_or_default();
    ParticipantThumbnailConfiguration {
        target_interval_seconds: Some(
            requested
                .target_interval_seconds
                .unwrap_or(DEFAULT_THUMBNAIL_INTERVAL_SECONDS),
        ),
        storage: Some(requested.storage.unwrap_or_else(default_thumbnail_storage)),
        recording_mode: Some(
            requested
                .recording_mode
                .unwrap_or_else(|| DEFAULT_THUMBNAIL_RECORDING_MODE.to_string()),
        ),
    }
}
```
(`state.rs:118`: `pub const DEFAULT_THUMBNAIL_RECORDING_MODE: &str = "DISABLED";`)

Reached from CreateStage — src/ops/create_stage.rs:214-227:
```
        let recording_configuration = crate::state::resolved_recording_configuration(
            &store,
            Self::NAME,
            input.auto_participant_recording_configuration,
```
and from UpdateStage — src/ops/update_stage.rs:176-183 (`resolved_recording_configuration(... Some(configuration) ...)`), which calls `effective_recording_configuration` (state.rs:229-231: `thumbnail_configuration: Some(effective_thumbnail_configuration(requested.thumbnail_configuration))`).

The sibling documented pairing this crate DOES enforce — src/ops/create_ingest_configuration.rs:108-116 and 217-229:
```
/// The model documents the pairing on both members:
/// `CreateIngestConfigurationRequest$ingestProtocol` ("If this is set to RTMP,
/// insecureIngest must be set to true") ... It is a documented requirement the
/// model carries no trait for ... so the handler enforces it and answers the modelled `ValidationException`.
...
        if input.ingest_protocol == CLEARTEXT_INGEST_PROTOCOL && !input.insecure_ingest {
            return Err(crate::state::validation_failed(
```
No equivalent check exists anywhere for the thumbnail pairing (`grep -n "targetIntervalSeconds|target_interval_seconds" src/` returns only state.rs:104/158/160 and the generated types).

Existing test coverage of the current behaviour — tests/all/stages.rs:1538-1559, row "thumbnails at the maximum interval, in both storage formats" sends `{"targetIntervalSeconds": 86_400, "storage": ["SEQUENTIAL", "LATEST"]}` with no `recordingMode` and asserts the stage reports `{"targetIntervalSeconds": 86400, "storage": [...], "recordingMode": "DISABLED"}`; the suite's stated purpose is "Each member a supplied recording configuration leaves out is reported at the default the model documents for it" (stages.rs:1460-1461), which says nothing about the pairing. Rows that pair an interval with `"recordingMode": "INTERVAL"` (stages.rs:1515-1537, 1580-1606) are unaffected by enforcing the rule.

## Steps to Reproduce

1. POST /CreateStorageConfiguration {"s3": {"bucketName": "recordings"}} and keep the returned storageConfiguration.arn
2. POST /CreateStage {"name": "thumbs", "autoParticipantRecordingConfiguration": {"storageConfigurationArn": "<arn>", "thumbnailConfiguration": {"targetIntervalSeconds": 86400}}} — recordingMode omitted, so it resolves to the documented default DISABLED
3. Observe HTTP 200 (no ValidationException) and that the response reports thumbnailConfiguration = {"targetIntervalSeconds": 86400, "storage": ["SEQUENTIAL"], "recordingMode": "DISABLED"} — an interval configured in the mode the model says it is not configurable in
4. POST /GetStage {"arn": "<stage arn>"} — the same interval is reported back, so the unconfigurable value was persisted
5. POST /UpdateStage {"arn": "<stage arn>", "autoParticipantRecordingConfiguration": {"storageConfigurationArn": "<arn>", "thumbnailConfiguration": {"targetIntervalSeconds": 120, "recordingMode": "DISABLED"}}} — the explicit DISABLED + interval combination is accepted too
6. Compare with POST /CreateIngestConfiguration {"ingestProtocol": "RTMP", "insecureIngest": false}, which the crate refuses with ValidationException for the analogous documented-but-unmodelled member pairing
