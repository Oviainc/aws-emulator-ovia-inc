Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items
Issue-Key: s3__stepfunctions.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items|_one_map_run#10|AssertionError|assert len(runs) == 1, f"expected exactly one map run for {exec_arn!r}, got {runs!r}"
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# Step Functions Distributed Map creates no map run: ListMapRuns answers empty for an execution containing a Distributed Map state, so a redrive cannot be targeted or observed

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMapRuns

## Description
One net-new root cause, grounded in a failing behavioral check that the replay report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: a Distributed Map state produces no MapRun, so ListMapRuns is empty ===
AWS: recorded behavior only for the end-to-end claim; the shapes are modeled. A state machine execution that enters a Distributed Map state creates exactly one MapRun child resource for that state. ListMapRuns(ExecutionArn) therefore returns exactly one MapRunListItem for such an execution, whose MapRunArn is the handle DescribeMapRun and RedriveExecution need; the map run's child executions run the inner workflow per item, and after a redrive only the items that failed re-run.
Emulator: ListMapRuns for the execution of a state machine whose definition contains a Distributed Map returns an empty list, so there is no map run to describe or redrive, and no child executions ran. This is consistent with the already-open finding that the interpreter does not run the workflow language (20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md, and 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md), but neither of those issues carries this check's replay key, and the specific missing artefact here is the MapRun resource and its listing rather than a Task state's execution.
Why: a client cannot discover, describe, or redrive a Distributed Map run at all. Redrive semantics — re-running exactly the failed children and leaving the succeeded ones alone — are unobservable, and any code that reads ListMapRuns to find the map run it just started gets an empty list and fails or loops.
Fix: the Step Functions interpreter/state layer — entering a Map state in DISTRIBUTED mode must materialise a MapRun resource bound to the parent execution, start one child execution per item, and record per-item status; ListMapRuns must serve those map runs for the parent execution arn, and RedriveExecution must re-run only the items whose last attempt failed.
Traces: check s3__sfn.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items

## Evidence
s3__sfn.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items:
  expected exactly one map run for the execution arn (9 children: 6 succeed, 3 fail; after RedriveExecution only the 3 failed children re-run and write their S3 markers again)
  actual   AssertionError: expected exactly one map run for 'arn:aws:states:us-west-2:000000000000:execution:sim2real-behav-sm-<id>:sim2real-behav-exec-<id>', got []
  (raised from _one_map_run -> ListMapRuns, before any redrive is attempted)

## Steps to Reproduce
Replay the behavioral suite and re-run check id s3__sfn.sfn-redrive-of-a-partly-failed-map-run-reruns-only-the-failed-items.

Minimal reproduction: put an items manifest in S3; CreateStateMachine with a Map state in DISTRIBUTED mode whose ItemReader reads that manifest and whose inner workflow writes one uniquely named S3 object per item (making 3 of 9 items fail); StartExecution and wait for it to reach a terminal state; call ListMapRuns(ExecutionArn=<the execution arn>) -> expect exactly one MapRunListItem. Then RedriveExecution and count the per-item writes -> expect the 3 failed items to have written twice and the 6 succeeded items once.
