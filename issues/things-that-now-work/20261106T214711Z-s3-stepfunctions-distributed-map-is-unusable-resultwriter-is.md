Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.sfn-distributed-map-over-real-s3-objects-writes-a-result-manifest s3__stepfunctions.sfn-distributed-map-without-start-execution-permission-fails-with-zero-children s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items
Issue-Key: s3__stepfunctions.sfn-distributed-map-over-real-s3-objects-writes-a-result-manifest|run#11|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: s3__stepfunctions.sfn-distributed-map-without-start-execution-permission-fails-with-zero-children|run#12|AssertionError|assert first["status"] == "FAILED", (
Issue-Key: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items|_wait_terminal#10|Timeout|time.sleep(interval_s)
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# s3__stepfunctions: Distributed Map is unusable — ResultWriter is refused as uninterpretable, the child StartExecution permission is never enforced, and a partly-failed map run cannot be redriven (3 causes)

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three net-new root causes behind 3 failing checks.

Cause 1 of 3: the `ResultWriter` member of a Distributed Map state is not interpreted, and instead of writing the result manifest the whole execution is failed.
AWS: a Map state in DISTRIBUTED mode with an `ItemReader` over S3 and a `ResultWriter` runs one child execution per item and writes the run's manifest (`manifest.json` plus SUCCEEDED/FAILED/PENDING result objects) under the configured bucket/prefix; the execution ends SUCCEEDED and DescribeMapRun reports the item counts (Smithy: DescribeMapRun -> {status: MapRunStatus, itemCounts, executionCounts, toleratedFailurePercentage, toleratedFailureCount, maxConcurrency} in services/service-com-amazonaws-sfn-awsstepfunctions; the ASL members themselves are recorded behavior only). AWS never fails an execution for using a valid ASL member.
Emulator: `status='FAILED'`, `error='States.Runtime'`, `cause="state 'Fan' carries the member 'ResultWriter', which this emulator does not interpret, so nothing was run for it"`. The cause is emulator-authored fixed text; the state name (`Fan`) and the member name (`ResultWriter`) are the only values taken from the definition.
Traces: s3__stepfunctions.sfn-distributed-map-over-real-s3-objects-writes-a-result-manifest.

Cause 2 of 3: a Distributed Map whose execution role cannot start its own child executions still SUCCEEDS instead of failing.
AWS: child executions of a DISTRIBUTED Map are started by Step Functions under the execution role, so a role without `states:StartExecution` on the state machine makes the map run fail — the execution ends FAILED with the authorization failure surfaced on the map run, and ListExecutions for that map run is empty (zero children). Recorded behavior only for the exact error name; the requirement is the non-success terminal status and zero children.
Emulator: the unprivileged execution ends `status='SUCCEEDED'` with `error=None` and `cause=None` — the child-start authorization is never evaluated, so a missing IAM grant is indistinguishable from a granted one.
Traces: s3__stepfunctions.sfn-distributed-map-without-start-execution-permission-fails-with-zero-children.

Cause 3 of 3: a partly-failed Distributed Map run can never be redriven to completion — the redrive scenario never settles.
AWS: RedriveExecution on an execution whose map run had failing children re-runs exactly the failed children from the step that failed, leaves the already-succeeded children untouched (their per-item side effects are not repeated), and DescribeExecution reports a non-zero `redriveCount` with a `redriveDate` and `redriveStatus` (modelled on DescribeExecution in the same service model).
Emulator: the check is killed at `Check timed out after 60.0s` — no terminal map run and no redrive outcome are ever observable.
Traces: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items.

Why (all three): a client relying on AWS's behavior cannot use Distributed Map at all. A valid definition is refused with a synthetic `States.Runtime` so no manifest is produced and Catch/Retry keyed on real States.* names never match; a run that AWS refuses for a missing IAM grant reports success, so a permission regression is invisible and the caller believes work was done that never happened; and a partially failed large-scale run cannot be recovered because redrive never completes.

Fix: the Distributed Map implementation in the sfn service crate's Map-state handler and map-run state — interpret `ItemReader`/`ResultWriter` (writing the manifest and per-status result objects to S3), start each child execution under the execution role and honour its `states:StartExecution` authorization (failing the map run with zero children when it is denied), and make map-run status/redrive terminal and observable through DescribeMapRun/ListExecutions/RedriveExecution. The open issue 20261025T053834Z-sfn-step-functions-distributed-map-creates-no-map-run.md records that ListMapRuns answers empty under the `sfn` heading; it does not carry any of these checks' replay keys.

## Evidence
Cause 1: expected `status="SUCCEEDED"` with the ResultWriter manifest written under the output prefix. Actual: `AssertionError: execution was 'FAILED': 'States.Runtime' "state 'Fan' carries the member 'ResultWriter', which this emulator does not interpret, so nothing was run for it"`.

Cause 2: expected the unprivileged execution `status="FAILED"` with zero child executions listed for the map run. Actual: `AssertionError: the unprivileged execution was 'SUCCEEDED', expected FAILED: None None` — no error, no cause.

Cause 3: expected a redrive that re-runs exactly the 3 failed items of 9 and leaves the 6 succeeded items' S3 writes at their original counts. Actual: `Error: Check timed out after 60.0s`.

## Steps to Reproduce
Cause 1: put N objects under a prefix in S3; CreateStateMachine (STANDARD) with a Map state in DISTRIBUTED mode, ItemReader over that prefix and a ResultWriter to an output prefix; StartExecution and poll DescribeExecution. Expected SUCCEEDED + manifest; observed FAILED States.Runtime naming ResultWriter.

Cause 2: create the same Distributed Map machine with an execution role that does NOT grant states:StartExecution on it; StartExecution and poll. Expected FAILED with ListExecutions for the map run empty; observed SUCCEEDED.

Cause 3: run a Distributed Map over 9 items where 3 children fail; after the run is terminal call RedriveExecution and re-read the per-item S3 write counts and DescribeExecution redriveCount. Expected only the 3 failed items re-run; observed the check never settles within 60 s.
