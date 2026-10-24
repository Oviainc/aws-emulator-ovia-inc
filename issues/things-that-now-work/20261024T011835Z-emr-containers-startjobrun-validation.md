Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# StartJobRun accepts a submission with no jobDriver, releaseLabel or executionRoleArn (and no jobTemplateId) and runs it

_Filed: 2026-10-24T01:18:35Z_
_Operation: StartJobRun | Class: validation_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected (AWS): a `StartJobRun` that references no job template must itself supply the values a run needs — `executionRoleArn`, `releaseLabel` and `jobDriver`. A request carrying only `clientToken` is rejected with HTTP 400 `ValidationException`; nothing is created.

Actual (emulator): the handler performs no such check. A body of just `{"clientToken": "..."}` is persisted as a `JobRunRecord` with `execution_role_arn: None`, `release_label: None`, `job_driver: None`, answered 200 with an id and ARN, and the clock then moves it PENDING -> RUNNING. `DescribeJobRun`/`ListJobRuns` therefore report a RUNNING job run that has no driver, no release label and no execution role — a state the real service can never produce.

## Evidence

Spec: `JobTemplateData` — the shape the model documents as "The values of StartJobRun API requests used in job runs started using the job template" — marks `executionRoleArn`, `releaseLabel` and `jobDriver` @required, which establishes that the service requires all three to run a job; they are optional on `StartJobRunRequest` only because `jobTemplateId` can supply them. `StartJobRun` models `ValidationException` (HTTP 400) for invalid parameters.

Handler: services/service-com-amazonaws-emrcontainers-awschicagowebservice/src/ops/start_job_run.rs lines 257-293 — the record is built straight from the (absent) optional inputs with no required-combination check, and no branch returns ValidationException for a driverless submission.

Tests: tests/all/virtual_cluster_job_run_loop.rs:569-572 asserts the opposite of AWS — a case labelled "no driver at all" (`{"clientToken":"job-tok-4"}`) is expected to succeed and round-trip through DescribeJobRun.

## Steps to Reproduce

1. POST /virtualclusters with {name, clientToken, containerProvider:{id,type:EKS}}; keep the cluster id.
2. POST /virtualclusters/{clusterId}/jobruns with body {"clientToken":"bare-1"} (no jobDriver, releaseLabel, executionRoleArn, jobTemplateId).
3. Observed: HTTP 200 with a job run id. Expected: HTTP 400 with x-amzn-errortype: ValidationException.
4. GET /virtualclusters/{clusterId}/jobruns/{id} after ~1s: observed state RUNNING with jobDriver/releaseLabel/executionRoleArn absent.
