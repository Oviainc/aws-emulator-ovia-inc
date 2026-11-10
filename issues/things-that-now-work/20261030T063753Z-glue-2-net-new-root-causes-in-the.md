Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.ml-task-run-timeout-transitions-to-timeout-state glue.ml-task-runs-ordering-most-recent-first
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.ml-task-run-timeout-transitions-to-timeout-state|run#9|AssertionError|assert final_status == "TIMEOUT", (
Issue-Key: glue.ml-task-runs-ordering-most-recent-first|run#14|AssertionError|assert run_ids.index(run_id_2) < run_ids.index(run_id_1), (
Issue-Commit: 2fe9ad840c0110bf567bc4da7d67a08009876174

# Glue: 2 net-new root causes in the machine-learning transform task-run surface — a task run never reaches TIMEOUT, and GetMLTaskRuns defaults to oldest-first

> _Note: this issue was identified against the code base at commit `2fe9ad840c0110bf567bc4da7d67a08009876174`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMLTaskRuns

## Description
Two net-new root causes in the Glue ML-transform task-run surface, both grounded in failing behavioral checks of the `glue` service. No open issue covers either: the existing ML-adjacent Glue issues concern job runs, column-statistics task runs and blueprint runs, not MLTaskRuns.

Cause 1 of 2: a machine-learning task run never settles to TIMEOUT, so the owning transform's Timeout is inert.
AWS: Smithy — CreateMLTransform.Timeout documents "The timeout of the task run for this transform in minutes. This is the maximum time that a task run for this transform can consume resources before it is terminated and enters TIMEOUT status. The default is 2,880 minutes (48 hours)." TaskStatusType enumerates STARTING, RUNNING, STOPPING, STOPPED, SUCCEEDED, FAILED, TIMEOUT, and GetMLTaskRun.Status reports "the current status of the requested task run". A task run of a transform whose Timeout has elapsed is therefore terminated in the TIMEOUT terminal status, distinct from FAILED (which is the status for a run Glue rejects on its input).
Emulator: a labeling-set-generation task run started on a transform created with Timeout=1 (minute) settles to the FAILED terminal status; GetMLTaskRun.Status never returns TIMEOUT for any run. Replay output: AssertionError: expected the 1-minute-timeout MLTaskRun to reach TIMEOUT, got FAILED.
Why: a client cannot tell a run that exhausted its resource budget (retry it, or raise Timeout on the transform) from a run Glue refused on bad input (fix the input). Operational automation and alarms keyed on the TIMEOUT status never fire, and every run looks like a hard failure.
Fix: the ML task-run state machine in the Glue service state — the settlement pass the readers invoke (`transform.settle_due_task_runs(now)` at services/service-com-amazonaws-glue-awsglue/src/ops/get_ml_task_runs.rs:208 and the equivalent call in the GetMLTaskRun handler) must compare the run's elapsed time against the owning MLTransform's Timeout (in minutes) and settle such a run to TIMEOUT, reserving FAILED for input-rejection.
Traces: glue.ml-task-run-timeout-transitions-to-timeout-state.

Cause 2 of 2: GetMLTaskRuns orders a transform's run history oldest-first when the request names no Sort.
AWS: Smithy — GetMLTaskRuns "Gets a list of runs for a machine learning transform… You can get a sortable, filterable list of machine learning task runs… This operation returns a list of historic runs and must be paginated"; Sort (TaskRunSortCriteria{Column, SortDirection}) is OPTIONAL. Recorded behavior only for the unsorted default: real Glue returns the run history most-recently-started first, so the run started second appears at a lower index than the run started first.
Emulator: with no Sort member, the listing comes back in ascending order — the first-started run precedes the later one. Replay output: AssertionError: GetMLTaskRuns must order the most recently started task run first.
Why: a client that reads the first page (or the first element) of GetMLTaskRuns to find the latest run of a transform gets the OLDEST run instead, and with a MaxResults-bounded page the newest run only appears on the last page — so polling the head of the listing never observes the run just started.
Fix: the default ordering used by the transform's run listing — `transform.task_runs_in_order(filter, sort)` as called at services/service-com-amazonaws-glue-awsglue/src/ops/get_ml_task_runs.rs:212 — must order by start time DESCENDING when the request carries no Sort criteria (an explicit Sort keeps deciding the order, and the pagination cursor must follow that same order).
Traces: glue.ml-task-runs-ordering-most-recent-first.

## Evidence
Cause 1 — glue.ml-task-run-timeout-transitions-to-timeout-state:
  Expected (AWS): GetMLTaskRun(TransformId, TaskRunId).Status == "TIMEOUT" for a labeling-set-generation run on an MLTransform created with Timeout=1 minute (Smithy: CreateMLTransform.Timeout — "terminated and enters TIMEOUT status"; TaskStatusType includes TIMEOUT).
  Actual (emulator): Status settles to "FAILED".
  Replay detail: File "check_glue_ml_task_run_timeout_transitions_to_timeout_state.py", line 64, in run — AssertionError: expected the 1-minute-timeout MLTaskRun to reach TIMEOUT, got FAILED.

Cause 2 — glue.ml-task-runs-ordering-most-recent-first:
  Expected (AWS): GetMLTaskRuns(TransformId) with no Sort returns TaskRuns most-recently-started first, i.e. index(second-started run) < index(first-started run).
  Actual (emulator): the two runs come back in start order (oldest first), so index(second-started run) > index(first-started run).
  Replay detail: File "check_glue_ml_task_runs_ordering_most_recent_first.py", line 48, in run — AssertionError: GetMLTaskRuns must order the most recently started task run first.

Both statuses are 200 responses; the divergence is in the response content, not the wire status.

## Steps to Reproduce
Cause 1:
1. glue.CreateDatabase + glue.CreateTable over a readable S3 location (CSV with id,name,category columns).
2. glue.CreateMLTransform(InputRecordTables=[that table], Parameters={TransformType: FIND_MATCHES, FindMatchesParameters:{PrimaryKeyColumnName: id}}, Role=<glue service role>, WorkerType=G.1X, NumberOfWorkers=2, Timeout=1, MaxRetries=0).
3. glue.StartMLLabelingSetGenerationTaskRun(TransformId, OutputS3Path=s3://<bucket>/labels/).
4. Poll glue.GetMLTaskRun(TransformId, TaskRunId) until Status is terminal.
   Observed: FAILED. Expected: TIMEOUT.

Cause 2:
1. glue.CreateMLTransform as above (no Timeout needed).
2. glue.StartMLLabelingSetGenerationTaskRun twice in sequence (OutputS3Path labels1/ then labels2/), waiting for each run to reach a terminal status.
3. glue.GetMLTaskRuns(TransformId) with no Sort member.
   Observed: [run1, run2]. Expected: [run2, run1] (most recently started first).
