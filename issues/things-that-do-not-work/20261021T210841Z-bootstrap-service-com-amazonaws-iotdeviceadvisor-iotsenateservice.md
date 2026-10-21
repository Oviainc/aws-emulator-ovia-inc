Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the iotdeviceadvisor emulator

_Filed: 2026-10-21T21:08:41Z_
_Operation: service-com-amazonaws-iotdeviceadvisor-iotsenateservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

All 14 operations of AWS IoT Core Device Advisor (`IotDeviceAdvisor`, restJson1, signing name `iotdeviceadvisor`) are unmodified scaffolds answering HTTP 501 `NotImplemented` and hold zero state. Implement a minimal, fully stateful first slice covering the primary resource lifecycle (suite definition) plus one suite run, so a realistic client script works end-to-end.

### Happy path to implement (in order)

1. **CreateSuiteDefinition** — `POST /suiteDefinitions`. Input: `suiteDefinitionConfiguration` (required; inner required members `suiteDefinitionName`, `rootGroup`, `devicePermissionRoleArn`, optional `devices`, `intendedForQualification`, `isLongDurationTest`, `protocol`), `tags`, `clientToken` (@idempotencyToken). Must mint and PERSIST a new definition: generate a UUID `suiteDefinitionId` (len 12..36), build `suiteDefinitionArn` as `arn:aws:iotdeviceadvisor:<region>:<account>:suitedefinition/<id>`, record `createdAt`/`lastModifiedAt`, store the whole configuration and tags, and set `latestVersion` to the first version string (e.g. `"1"`, len 2..255 means version strings must be at least 2 chars — use e.g. `"v1"`/`"1.0"` consistently with what GetSuiteDefinition returns). Response echoes `suiteDefinitionId`, `suiteDefinitionArn`, `suiteDefinitionName` (taken from the STORED config) and `createdAt`.
2. **GetSuiteDefinition** — `GET /suiteDefinitions/{suiteDefinitionId}` (+ optional `suiteDefinitionVersion` query). Must look the record up in state and return `suiteDefinitionId`, `suiteDefinitionArn`, `suiteDefinitionVersion`, `latestVersion`, the exact `suiteDefinitionConfiguration` that was created (or updated), `createdAt`, `lastModifiedAt`, `tags`. Unknown id ⇒ `ResourceNotFoundException` (HTTP 404); malformed id (outside len 12..36) ⇒ `ValidationException` (HTTP 400).
3. **ListSuiteDefinitions** — `GET /suiteDefinitions`. Must return a `suiteDefinitionInformationList` projected from stored definitions (`suiteDefinitionId`, `suiteDefinitionName`, `defaultDevices` from the config's `devices`, `intendedForQualification`, `isLongDurationTest`, `protocol`, `createdAt`), honouring `maxResults` (range 1..50) and round-tripping `nextToken` (len 0..2000) so paging terminates with `nextToken` absent on the last page.
4. **StartSuiteRun** — `POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns`. Input `suiteRunConfiguration` (required; inner required `primaryDevice`, optional `selectedTestList`, `parallelRun`), optional `suiteDefinitionVersion`, `tags`. Must create and persist a run keyed by `(suiteDefinitionId, suiteRunId)`: new UUID `suiteRunId`, `suiteRunArn`, `createdAt`, `endpoint` (len 45..75, same value GetEndpoint returns), initial `status = PENDING` then `RUNNING`. Unknown definition ⇒ `ResourceNotFoundException`; starting a second run while one is `RUNNING`/`PENDING` for the same definition ⇒ `ConflictException` (HTTP 400).
5. **GetSuiteRun** — `GET /suiteDefinitions/{suiteDefinitionId}/suiteRuns/{suiteRunId}`. Must return the persisted run: `suiteDefinitionId`, `suiteDefinitionVersion`, `suiteRunId`, `suiteRunArn`, the stored `suiteRunConfiguration`, `startTime`, `endTime` (only once terminal), `status`, `errorReason`, `tags`, and a `testResult` consistent with `status`. Unknown ids ⇒ `ResourceNotFoundException`.
6. **StopSuiteRun** — `POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns/{suiteRunId}/stop`. Must mutate the stored run to `STOPPING`/`STOPPED` (so a following GetSuiteRun observes the new status and an `endTime`), empty 200 body. Unknown run ⇒ `ResourceNotFoundException`. Stopping an already-terminal run must not resurrect it.
7. **DeleteSuiteDefinition** — `DELETE /suiteDefinitions/{suiteDefinitionId}`. Must remove the definition (and its runs) from state, empty 200 body; a subsequent GetSuiteDefinition must return `ResourceNotFoundException`.

No canned, hardcoded or echoed-input responses: every one of these must compute its response from persisted state, and a value returned by a create must be observable by a later read in the same process. A random id with no backing store counts as a failure.

### State Model

A service-level store behind the existing state lock, e.g.:

```
IotDeviceAdvisorState {
    // suiteDefinitionId -> definition
    definitions: Map<String, SuiteDefinition>,
    // suiteRunId -> run (plus index suiteDefinitionId -> [suiteRunId])
    runs: Map<String, SuiteRun>,
    // resourceArn -> tags, for the tagging ops
    tags: Map<String, Map<String, String>>,
}

SuiteDefinition {
    id: String,                 // UUID, 12..36
    arn: String,                // arn:aws:iotdeviceadvisor:<region>:<account>:suitedefinition/<id>
    latest_version: String,     // bumped by UpdateSuiteDefinition
    versions: Map<String, SuiteDefinitionConfiguration>,  // version -> config snapshot
    created_at: Timestamp,
    last_modified_at: Timestamp,
    tags: Map<String, String>,
}

SuiteRun {
    id: String,                 // UUID, 12..36
    arn: String,
    suite_definition_id: String,
    suite_definition_version: String,
    configuration: SuiteRunConfiguration,   // primaryDevice, selectedTestList, parallelRun
    status: SuiteRunStatus,     // PENDING|RUNNING|STOPPING|STOPPED|PASS|FAIL|ERROR|CANCELED|PASS_WITH_WARNINGS
    test_result: TestResult,    // per selected test/group
    created_at / started_at / end_at: Option<Timestamp>,
    error_reason: Option<String>,
    endpoint: String,
    tags: Map<String, String>,
}
```

Out of scope for this first slice (stay 501, filed separately later): `UpdateSuiteDefinition`, `GetEndpoint`, `GetSuiteRunReport`, `ListSuiteRuns`, `TagResource`, `UntagResource`, `ListTagsForResource` — though the state model above already reserves room for them.

## Evidence

`services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/OPERATIONS.md` lists all 14 operations with routes, input/output members and errors (restJson1, SDK id `IotDeviceAdvisor`, endpoint prefix `api.iotdeviceadvisor`); field names and requiredness above are taken from it and from `smithy_query` on `SuiteDefinitionConfiguration`, `SuiteDefinitionInformation`, `SuiteRunConfiguration`, `SuiteRunInformation`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/` matches all 14 handler files (create_suite_definition.rs, get_suite_definition.rs, list_suite_definitions.rs, start_suite_run.rs, get_suite_run.rs, stop_suite_run.rs, delete_suite_definition.rs, plus the 7 out-of-scope ops), each returning `crate::scaffolded::Scaffolded::not_implemented(..)` ⇒ HTTP 501, so no state exists anywhere in the crate. No tests assert any of this behavior.

## Steps to Reproduce

1. Point an AWS SDK/CLI client at the emulator with service `iotdeviceadvisor`.
2. CreateSuiteDefinition with suiteDefinitionConfiguration={suiteDefinitionName:'demo-suite', rootGroup:'', intendedForQualification:true, devicePermissionRoleArn:'arn:aws:iam::123456789012:role/DeviceAdvisorRole', devices:[{thingArn:'arn:aws:iot:us-east-1:123456789012:thing/demo-thing'}]} — expect 200 with a fresh suiteDefinitionId, suiteDefinitionArn, suiteDefinitionName='demo-suite' and createdAt (today: 501).
3. GetSuiteDefinition with that suiteDefinitionId — expect 200 echoing the stored configuration verbatim plus latestVersion, createdAt, lastModifiedAt.
4. ListSuiteDefinitions with maxResults=1 — expect the created definition in suiteDefinitionInformationList and nextToken handling that terminates.
5. StartSuiteRun on the definition with suiteRunConfiguration={primaryDevice:{thingArn:'arn:aws:iot:us-east-1:123456789012:thing/demo-thing'}} — expect 200 with suiteRunId, suiteRunArn, createdAt, endpoint.
6. GetSuiteRun with (suiteDefinitionId, suiteRunId) — expect 200 with status PENDING/RUNNING and the stored suiteRunConfiguration.
7. StopSuiteRun on the same run, then GetSuiteRun again — expect the status to have moved to STOPPING/STOPPED with an endTime.
8. GetSuiteRun with a random unknown suiteRunId — expect ResourceNotFoundException (HTTP 404), not 501 or 200.
9. DeleteSuiteDefinition, then GetSuiteDefinition on the same id — expect ResourceNotFoundException (HTTP 404) and the definition gone from ListSuiteDefinitions.
