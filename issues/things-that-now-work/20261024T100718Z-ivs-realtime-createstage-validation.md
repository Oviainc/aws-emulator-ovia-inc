Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ca029f426fda2d51f4b9c87edd69d4539a13e33a

# CreateStage accepts an autoParticipantRecordingConfiguration.storageConfigurationArn that names no storage configuration

_Filed: 2026-10-24T10:07:18Z_
_Operation: CreateStage | Class: validation_

> _Note: this issue was identified against the code base at commit `ca029f426fda2d51f4b9c87edd69d4539a13e33a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AutoParticipantRecordingConfiguration$storageConfigurationArn` is "ARN of the StorageConfiguration resource to use for individual participant recording" — a reference to a resource of this same service, which CreateStorageConfiguration/GetStorageConfiguration already hold in this emulator's store. CreateStage models ValidationException (400) and no ResourceNotFoundException, so the reference check is a validation refusal.

Expected: CreateStage with a well-formed but dangling `storageConfigurationArn` (one that no CreateStorageConfiguration in this account/Region ever produced, or one that was deleted) is refused with ValidationException (400) and no stage is created. Only the empty string — the documented "no storage configuration" value — is accepted without naming a resource.

Actual: the ARN is copied into the stage record untouched. `crate::state::effective_recording_configuration` applies the member defaults and the documented empty-string reset but performs no lookup against `store.storage_configurations`, so a stage can be created bound to a storage configuration that does not exist; GetStage/ListStages then report that configuration back as if recording were correctly wired. A client that mistypes the ARN (or deletes the configuration first) never learns of it. The identical unchecked path is taken by UpdateStage, which calls the same seam (filed separately).

## Evidence

Spec: smithy_query(query="shape", shape="AutoParticipantRecordingConfiguration") — storageConfigurationArn is @required inside the structure, pattern `^$|^arn:aws:ivs:…:storage-configuration/…$`, documented as the StorageConfiguration "resource to use"; smithy_query(query="operation", operation="CreateStage") — ValidationException (400) is the only client error that fits a bad reference.
Precedent in this service: the open issue 20261024T034553Z-ivs-realtime-createingestconfiguration-validation accepts the same reasoning for CreateIngestConfiguration's dangling `stageArn`, and the sibling IVS service has a recorded AWS refusal for an unresolvable recording destination (issue 20261023T004057Z, Cause 4).
Handler: src/ops/create_stage.rs `handle` lines 245-263 (`auto_participant_recording_configuration: crate::state::effective_recording_configuration(input.auto_participant_recording_configuration)`); src/state.rs `effective_recording_configuration` lines 190-211 — no access to `state.storage_configurations`.
Tests: tests/all/stages.rs rows (e.g. lines 921-943, 1299, 1462-1698) pass fabricated ARNs such as `arn:aws:ivs:us-east-1:000000000000:storage-configuration/abcd1234` and assert HTTP 200, so the suite currently codifies acceptance; those rows would create a real storage configuration first once the check lands.

## Steps to Reproduce

1. POST /CreateStage {"name":"rec","autoParticipantRecordingConfiguration":{"storageConfigurationArn":"arn:aws:ivs:us-east-1:000000000000:storage-configuration/abcd1234"}} without ever creating that storage configuration
2. Expected: HTTP 400 ValidationException naming autoParticipantRecordingConfiguration.storageConfigurationArn, and no stage created
3. Actual: HTTP 200; the stage is persisted and GetStage reports the dangling storageConfigurationArn back
4. Variant: POST /CreateStorageConfiguration, POST /CreateStage naming its ARN, POST /DeleteStorageConfiguration (once implemented) — the stage keeps pointing at a configuration that no longer exists
