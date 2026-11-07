Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.list-notebook-executions-ordering-and-status-filter
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.list-notebook-executions-ordering-and-status-filter|_run_execution_until#7|AssertionError|raise AssertionError(
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# EMR notebook executions never run: a healthy notebook ends FAILED with an emulator-authored reason, so the ListNotebookExecutions Status filter can never partition FINISHED from FAILED (1 cause)

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartNotebookExecution

## Description
ONE root cause. Cause 1 of 1: a notebook execution is recorded and then driven straight to the terminal FAILED state without the notebook ever being run, so no execution can reach FINISHED and the Status-filtered listing can never be exercised.

AWS: StartNotebookExecution returns a NotebookExecutionId (Smithy: StartNotebookExecution input requires ExecutionEngine + ServiceRole, output NotebookExecutionId: XmlStringMaxLen256; modeled errors InvalidRequestException 400 / InternalServerException 500). The execution then advances through the modeled lifecycle (Smithy: NotebookExecutionStatus = START_PENDING, STARTING, RUNNING, FINISHING, FINISHED, FAILING, FAILED, STOP_PENDING, STOPPING, STOPPED): a notebook whose cells all succeed reaches FINISHED and its output notebook is written to OutputNotebookS3Location, while only a notebook that raises reaches FAILED, with DescribeNotebookExecution.NotebookExecution.LastStateChangeReason carrying the engine's own reason for that failure. ListNotebookExecutions then partitions the two by Status (Smithy: ListNotebookExecutions.Status -> NotebookExecutionStatus, output NotebookExecutions: NotebookExecutionSummaryList) and returns a stable order across consecutive unfiltered calls. The FINISHED outcome for a succeeding notebook is recorded behavior; the recording shows the execution polled to FINISHED.

Emulator: the execution reaches terminal Status 'FAILED' almost immediately after StartNotebookExecution, with LastStateChangeReason set to the emulator-authored sentence 'This emulator records notebook executions but runs no notebook, so the execution ended without running.' That sentence is wholly emulator text — AWS's reason for a FAILED execution is produced by the notebook engine and names the cell/exception that failed; the fixed part of AWS's contract here is the STATE (FINISHED for a notebook that succeeds), not any message. Because the succeeding notebook never reaches FINISHED, the check's positive side never materialises and the Status=FINISHED / Status=FAILED partition and the order-stability assertions are never reached.

Why: a client that submits an EMR notebook execution and waits on it is told its healthy notebook FAILED, gets no output notebook in S3, and cannot use Status as a filter (every execution lands in the FAILED bucket), so run-to-completion, status filtering and ordering logic built on EMR notebooks all break.

Fix: the notebook-execution lifecycle behind StartNotebookExecution / DescribeNotebookExecution / ListNotebookExecutions in the emr service — the execution state machine (it must occupy STARTING/RUNNING/FINISHING and settle FINISHED for a notebook that succeeds, reserving FAILED for one that raises) and the output-notebook write, plus the Status filter in the ListNotebookExecutions handler once both terminal states are reachable.

Note: open issue 20261105T093917Z-emr-6-net-new-root-causes-the-emr.md describes the same 'runs no notebook' family as its Cause 3 and lists this check id among its traces, but its recorded replay key for this check is a _poll_until timeout, not the AssertionError this replay produced in _run_execution_until; the replay report did not mark this failure as owned by it, so it is filed here and that issue is named rather than duplicated silently.

Traces: emr.list-notebook-executions-ordering-and-status-filter.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emr: 77/216 passed).

[mismatch] emr.list-notebook-executions-ordering-and-status-filter:
  File "check_emr_list_notebook_executions_ordering_and_status_filter.py", line 33, in run
    ok_id, ok_final = _run_execution_until(ctx, emr, cluster_id, service_role, bucket, ok_key, "ok", "FINISHED")
  File line 114, in _run_execution_until
  AssertionError: ok notebook execution never reached FINISHED after 2 attempts, last execution=ex-<id> last status=FAILED reason='This emulator records notebook executions but runs no notebook, so the execution ended without running.'

Expected: DescribeNotebookExecution for the notebook whose single cell is `print(1 + 1)` reports Status FINISHED (after passing through STARTING/RUNNING/FINISHING), and only the sibling notebook whose cell raises reports FAILED with the engine's reason.
Actual: both executions report Status FAILED; the FAILED reason is the fixed emulator sentence quoted above (no variable part drawn from the request or the notebook), so Status=FINISHED lists nothing.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Replay simparity-behavioral-aws-v1 and read the '### emr' section.
3. Create an S3 bucket, upload a notebook whose only cell is `print(1 + 1)`, and RunJobFlow a keep-alive cluster with the EMR service role / EC2 instance profile; wait for the cluster to report WAITING or RUNNING.
4. Call emr StartNotebookExecution with ExecutionEngine={'Id': <cluster id>}, the service role, and NotebookS3Location pointing at that notebook.
5. Poll emr DescribeNotebookExecution on the returned NotebookExecutionId — expect Status to advance to FINISHED; observe Status 'FAILED' with LastStateChangeReason 'This emulator records notebook executions but runs no notebook, so the execution ended without running.'
6. Call emr ListNotebookExecutions(Status='FINISHED', ExecutionEngineId=<cluster id>) — expect the succeeding execution; observe an empty partition (replay emr.list-notebook-executions-ordering-and-status-filter).
