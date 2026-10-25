Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9d4ea5e78ce612465906f9a67fc5f9d068061c14

# recordParticipantReplicas defaults two different ways: true when no storage configuration is named, false when one is

_Filed: 2026-10-25T00:21:15Z_
_Operation: CreateStage | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9d4ea5e78ce612465906f9a67fc5f9d068061c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::effective_recording_configuration` is the single seam `CreateStage` and `UpdateStage` both use to fill in the documented defaults of `autoParticipantRecordingConfiguration`. It has two branches, and they disagree about the SAME member:

* When `storageConfigurationArn` is empty (or the whole object is absent) it returns `default_recording_configuration()`, which sets `record_participant_replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS` — the constant the crate defines as `true`, quoting the model's own "Default: `true`".
* When `storageConfigurationArn` names a storage configuration — the only case in which individual-participant recording is actually wired up, and therefore the only case in which the member has any effect — it copies `requested.record_participant_replicas` straight through. That field is `bool` (not `Option<bool>`) because of the Smithy `@default false`, so a request that never mentioned the member arrives as `false` and the stage is persisted and reported with `recordParticipantReplicas: false`.

So `CreateStage {"autoParticipantRecordingConfiguration":{"storageConfigurationArn":"<arn>"}}` reports `recordParticipantReplicas: false`, while `CreateStage {}` reports `recordParticipantReplicas: true`, for the one member whose model doc states a single default. The crate's own constant, with the model sentence quoted above it, is never reached on the branch where it matters. `UpdateStage` inherits the same split because it calls the same seam.

## Evidence

Smithy, `AutoParticipantRecordingConfiguration$recordParticipantReplicas` (`smithy_query(query="shape", shape="AutoParticipantRecordingConfiguration")`):
> "Optional field to disable replica participant recording. If this is set to `false` when a participant is a replica, replica participants are not recorded. Default: `true`."

The crate records that same sentence and turns it into a constant — services/.../src/state.rs:85-89:
```
/// Whether replica participants are recorded, when the configuration does not say.
///
/// Documented on `AutoParticipantRecordingConfiguration$recordParticipantReplicas`:
/// "Optional field to disable replica participant recording. … Default: `true`."
pub const DEFAULT_RECORD_PARTICIPANT_REPLICAS: bool = true;
```

Branch that applies it — services/.../src/state.rs:164-172:
```
pub fn default_recording_configuration() -> AutoParticipantRecordingConfiguration {
    AutoParticipantRecordingConfiguration {
        ...
        record_participant_replicas: DEFAULT_RECORD_PARTICIPANT_REPLICAS,
    }
}
```

Branch that does NOT — services/.../src/state.rs:197-213:
```
    if requested.storage_configuration_arn.is_empty() {
        return default_recording_configuration();
    }
    AutoParticipantRecordingConfiguration {
        storage_configuration_arn: requested.storage_configuration_arn,
        ...
        record_participant_replicas: requested.record_participant_replicas,
    }
```

Both handlers go through it: services/.../src/ops/create_stage.rs:216 (`crate::state::resolved_recording_configuration(&store, Self::NAME, input.auto_participant_recording_configuration)`) and services/.../src/ops/update_stage.rs:176 (same call), the latter commented "Through the same seam `CreateStage` uses, so an updated stage reports the configuration's documented defaults exactly as a created one does".

Test coverage, both sides, in one file. services/.../tests/all/stages.rs:1248-1257 quotes the model and asserts `true`:
```
/// `recordParticipantReplicas` "Default: true", ...
        "recordParticipantReplicas": true,
```
services/.../tests/all/stages.rs:949-955 pins `false` for the storage-configuration case with the stated reason that the framework materialised the Smithy `@default`:
```
    // `recordParticipantReplicas` carries `@default: false`, so the framework has
    // already materialised `false` by the time the handler sees the request.
    let stored_recording = json!({
        "storageConfigurationArn": storage.as_str(),
        ...
        "recordParticipantReplicas": false,
```
The quoted model sentence ("Default: `true`") contradicts that stated reason, and the crate's own `DEFAULT_RECORD_PARTICIPANT_REPLICAS` plus stages.rs:1257 already take the model's side on the other branch.

## Steps to Reproduce

1. `CreateStorageConfiguration {"name":"rec","s3":{"bucketName":"b"}}` and keep the returned `storageConfiguration.arn`.
2. `CreateStage {"name":"a","autoParticipantRecordingConfiguration":{"storageConfigurationArn":"<that arn>"}}`.
3. Read `stage.autoParticipantRecordingConfiguration.recordParticipantReplicas` in the response (or via `GetStage`): it is `false`.
4. `CreateStage {"name":"b"}` (no recording configuration at all) and read the same member: it is `true`.
5. One member of one shape has two different defaults depending on a sibling member; the model documents exactly one ("Default: `true`"), and `crate::state::DEFAULT_RECORD_PARTICIPANT_REPLICAS` is that value but is unreachable from step 2.
