Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2b443a1d32fe9865f4d2b32010552c8e62eefe3f

# statusDetails names are bounded at 1024 characters, eight times the model's 128-character DetailsKey maximum

_Filed: 2026-10-28T15:14:36Z_
_Operation: UpdateJobExecution | Class: validation_

> _Note: this issue was identified against the code base at commit `2b443a1d32fe9865f4d2b32010552c8e62eefe3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateJobExecution`'s `statusDetails` member targets `DetailsMap`, whose KEY is the model shape `DetailsKey`: `@length {min: 1, max: 128}` with `@pattern ^[a-zA-Z0-9:_-]+$`. The only key check the crate applies is a 1024-character ceiling (`MAX_STATUS_DETAIL_NAME_LEN`), so a name of 129..1024 characters is accepted and persisted on the execution, and no pattern test is applied at all. The crate's own test proves this is what a client sees rather than a check the framework performs ahead of the handler: it submits a name of exactly `MAX_STATUS_DETAIL_NAME_LEN` = 1024 characters through `UpdateJobExecution` and asserts HTTP 200 plus the map echoed back in `executionState.statusDetails`.

The 1024 ceiling rests on a comment that misstates this model. `MAX_STATUS_DETAIL_NAME_LEN`'s doc says the model documents a maximum for the VALUE only — but `DetailsValue` is the member with NO declared maximum (`@length {min: 1}`, the 1,024 figure coming from the `JobExecution$statusDetails` prose), while `DetailsKey` is the one that DOES carry a modelled maximum, of 128. The two bounds are therefore swapped relative to the model: the value is capped by a prose number and the key, which has a real `@length` trait, is capped eight times looser than the trait allows.

The same helper is the one `StartNextPendingJobExecution` calls directly (`src/ops/start_next_pending_job_execution.rs:197`), so both operations that accept `statusDetails` store over-long names; `UpdateJobExecution` is named here as the operation whose handler the pinning test drives.

## Evidence

The model's constraint on the map KEY — `smithy_query(query="shape", shape="DetailsMap,DetailsKey")`:
```
"DetailsMap": {"type": "map", "key": {"target": "DetailsKey", "length": {"min": 1, "max": 128}, "pattern": "^[a-zA-Z0-9:_-]+$"}, "value": {"target": "DetailsValue", "length": {"min": 1}, "pattern": "^[^\\p{C}]+$"}}
"DetailsKey": {"type": "string", "length": {"min": 1, "max": 128}, "pattern": "^[a-zA-Z0-9:_-]+$"}
```
Note `DetailsValue` carries NO max, so the crate's claim below is inverted.

The constant and the comment that justifies it — services/common/src/iot_job_executions.rs:189-194:
```
/// "The maximum length of the value in the name/value pair is 1,024 characters"
/// — every member that carries `DetailsMap`.
pub const MAX_STATUS_DETAIL_VALUE_LEN: usize = 1_024;
/// The model documents a maximum for the VALUE only; the same ceiling is applied to
/// the NAME so a record cannot be grown through long keys either.
pub const MAX_STATUS_DETAIL_NAME_LEN: usize = 1_024;
```

The only key check performed — services/common/src/iot_job_executions.rs:108-114 (no 128 limit, no pattern test):
```
    for (name, value) in details {
        let name_len = name.chars().count();
        if name_len > MAX_STATUS_DETAIL_NAME_LEN {
            return Err(format!(
                "a statusDetails name of {name_len} characters is over the {MAX_STATUS_DETAIL_NAME_LEN} character maximum"
            ));
        }
```
reached from the handler at services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:323:
```
        if let Err(reason) = record.try_replace_status_details(input.status_details.as_ref()) {
```
(`try_replace_status_details` calls `check_status_details` first — services/common/src/iot_job_executions.rs:434.)

The crate's own test pinning a 1024-character NAME as accepted, which is the proof the 128-character `@length` is not applied anywhere in the request walk — tests/all/job_execution_lifecycle.rs:1280-1307:
```
    let longest_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN);
...
        (
            "a name of exactly the maximum length",
            json!({longest_name: "v"}),
        ),
...
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            body["executionState"]["statusDetails"], details,
            "case {case:?}: expected the details to be stored, got {body}"
        );
```
and its sibling row refusing only at 1025 — tests/all/job_execution_lifecycle.rs:1323 `let over_long_name = "n".repeat(MAX_STATUS_DETAIL_NAME_LEN + 1);`. Test coverage of this rule is exactly `status_details_are_bounded_and_an_over_bound_map_changes_nothing` (tests/all/job_execution_lifecycle.rs:1271), whose doc states the rule as "each name and value within its own maximum length" — the name's "own maximum" per the model being 128, not 1024.

## Steps to Reproduce

1. Register a command in the shared IoT command index and POST /command-executions with a valid thing `targetArn` to get an `executionId` for thing `laser-1`.
2. POST /things/laser-1/jobs/{executionId} with body `{"status": "IN_PROGRESS", "statusDetails": {"<200 characters of n>": "v"}, "includeJobExecutionState": true}`.
3. Observe HTTP 200 and `executionState.statusDetails` echoing the 200-character name, which is then persisted — although `DetailsMap`'s key shape `DetailsKey` declares `@length {min: 1, max: 128}`.
4. GET /things/laser-1/jobs/{executionId} and observe the over-long name reported back as stored state.
5. Repeat with a name containing a character outside `^[a-zA-Z0-9:_-]+$` (for example `step name`) and observe it is likewise accepted — `check_status_details` applies no pattern test.
