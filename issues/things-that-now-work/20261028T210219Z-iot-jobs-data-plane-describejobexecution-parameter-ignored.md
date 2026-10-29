Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 8c3df3565db28ea355f1b3b898224a3d8ff548b1

# DescribeJobExecution silently drops executionNumber when jobId is $next, answering 200 with an execution the request did not name

_Filed: 2026-10-28T21:02:19Z_
_Operation: DescribeJobExecution | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `8c3df3565db28ea355f1b3b898224a3d8ff548b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DescribeJobExecution` passes `input.execution_number` into `JobExecutionStore::resolve_record`, but that lookup only honours it on the non-`$next` branch. When `jobId` is the literal `$next`, `resolve_index` calls `next_pending_index(thing_name)` and the `execution_number` argument is discarded, so `GET /things/{thingName}/jobs/$next?executionNumber=7` returns the thing's next pending execution whatever its `executionNumber` is.

The same handler, for the same `executionNumber` value, behaves the opposite way on a named `jobId`: `execution_index` filters on it and an `executionNumber` the thing holds no execution for is a 404 `ResourceNotFoundException` (pinned by this crate's own test). So one request member is a selector on one path of a single handler and is dropped on the other, and the `$next` path answers 200 reporting `executionNumber: 1` for a request that asked for execution 7.

## Evidence

The handler's own header states the rule the `$next` path breaks — src/ops/describe_job_execution.rs:5-7:
```
//! Reports the persisted execution `jobId` names on the thing — with `$next`
//! resolving to the thing's next pending execution and `executionNumber`
//! selecting one.
```
and it does pass the member in — src/ops/describe_job_execution.rs:190-191:
```
        let Some(record) =
            state.resolve_record(&input.thing_name, &input.job_id, input.execution_number)
```
The lookup drops it on the `$next` branch — services/common/src/iot_job_executions.rs:585-596:
```
    pub fn resolve_index(
        &self,
        thing_name: &str,
        job_id: &str,
        execution_number: Option<i64>,
    ) -> Option<usize> {
        if job_id == NEXT_JOB_ID {
            self.next_pending_index(thing_name)
        } else {
            self.execution_index(thing_name, job_id, execution_number)
        }
    }
```
(`next_pending_index` takes only `thing_name` — services/common/src/iot_job_executions.rs:552.)

The other branch of the same handler treats the identical value as a selector and 404s on it — tests/all/job_execution_lifecycle.rs:913-929:
```
        (
            "an executionNumber that does not exist",
            known,
            id.clone(),
            "?executionNumber=7",
            json!({"executionNumber": 7}),
        ),
...
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
```
No test in tests/all combines `$next` with `executionNumber` (grep for `executionNumber|\$next` over tests/all shows the two never appear in one request).

Model text for the member (smithy_query query="doc" shape="DescribeJobExecutionRequest" member="executionNumber"): "Optional. A number that identifies a particular job execution on a particular device. If not specified, the latest job execution is returned." — i.e. the member names a particular execution, and the `$next` path answers without consulting it.

The handler also admits an unfiled gap for exactly this class of request — src/ops/describe_job_execution.rs:17-20:
```
//! `InvalidRequestException` (400)
//! is still not produced here: the request combinations the real service refuses are
//! filed as their own issue along with the rest of the long tail (certificate
//! validation, throttling).
```

## Steps to Reproduce

1. Register a command and POST /command-executions for thing `laser-1` so the thing holds exactly one QUEUED execution (its `executionNumber` is `FIRST_EXECUTION_NUMBER` = 1).
2. GET /things/laser-1/jobs/{executionId}?executionNumber=7 — observe 404 ResourceNotFoundException "Job {executionId} not found for laser-1" (the named-jobId path filters on executionNumber).
3. GET /things/laser-1/jobs/$next?executionNumber=7 — observe 200 with `execution.executionNumber = 1`: the same executionNumber is dropped and an execution the request did not name is reported.
4. Compare with UpdateJobExecution, which routes the member through `execution_index` and therefore honours it on every request it serves.
