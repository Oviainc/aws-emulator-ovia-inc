Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.session-lifecycle-start-get-terminate emr.session-terminate-then-get-reflects-terminated-state emr.list-sessions-ordering-newest-first emr.e2e-studio-session-full-flow emr.emr-run-step-spark emr.list-notebook-executions-ordering-and-status-filter
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.session-lifecycle-start-get-terminate|_run_job_flow_with_retry#2|Timeout|raise
Issue-Key: emr.session-terminate-then-get-reflects-terminated-state|_run_job_flow_with_retry#3|Timeout|time.sleep(interval_s)
Issue-Key: emr.list-sessions-ordering-newest-first|_run_job_flow_with_role_propagation_retry#3|Timeout|time.sleep(interval_s)
Issue-Key: emr.e2e-studio-session-full-flow|_retry_on_client_error#15|Timeout|time.sleep(interval_s)
Issue-Key: emr.emr-run-step-spark|_poll_until#16|Timeout|_t.sleep(interval_s)
Issue-Key: emr.list-notebook-executions-ordering-and-status-filter|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Commit: 8ffd4f499c6d6f6b1d86ee4455070f9416ebee70

# EMR: 3 net-new root causes — the Spark-Connect session lifecycle, a Spark step's data plane and a notebook execution never advance, so 6 checks are cut off at the harness budget

> _Note: this issue was identified against the code base at commit `8ffd4f499c6d6f6b1d86ee4455070f9416ebee70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSession/GetSession/ListSessions/DescribeStep/DescribeNotebookExecution

## Description
Six EMR behavioral checks neither pass nor return a graded answer: they are killed by the harness after 60 s while still polling an EMR state the emulator never moves. Nothing in these checks waits on wall-clock time for its own sake — every wait is on an EMR state transition — and 103 checks of this same suite complete inside the same budget, so a 60 s cut-off here means the emulator parked the resource in a state it never leaves (or spins the check's own RunJobFlow retry loop by refusing the call). Three distinct root causes, all in the EMR emulator.

Cause 1 of 3: a session-enabled cluster never becomes a usable Spark Connect session. The four checks emr.session-lifecycle-start-get-terminate, emr.session-terminate-then-get-reflects-terminated-state, emr.list-sessions-ordering-newest-first and emr.e2e-studio-session-full-flow all do the same thing: RunJobFlow with SessionEnabled=true, ReleaseLabel=emr-spark-8.0.0, Applications=[Spark] into a private subnet, wait for the cluster to reach WAITING/RUNNING, StartSession, then poll GetSession for a running state and (for two of them) for TERMINATED after TerminateSession. Each wraps RunJobFlow in its own retry-on-ClientError loop (120 s / 320 s / N attempts), so a refusal of RunJobFlow is indistinguishable from a hang: the check spins inside the retry until the harness kills it.
AWS: RunJobFlow accepts emr-spark-8.0.0 with SessionEnabled=true and the cluster progresses STARTING -> BOOTSTRAPPING -> RUNNING/WAITING (Smithy: ClusterState = STARTING|BOOTSTRAPPING|RUNNING|WAITING|TERMINATING|TERMINATED|TERMINATED_WITH_ERRORS). StartSession then returns Id/ClusterId/Arn/State and the session walks the modeled state machine (Smithy: SessionState = SUBMITTED|STARTING|STARTED|IDLE|BUSY|TERMINATING|TERMINATED|FAILED; Session.StartedAt is documented as "the date and time that the session entered the STARTED state"), so GetSession reaches STARTED/IDLE/BUSY on its own and, after TerminateSession (whose output State is a required SessionState), reaches TERMINATED while the session stays readable. ListSessions returns the cluster's sessions newest-created first.
Emulator: none of that terminal condition is ever observed inside the budget — either RunJobFlow refuses the session-enabled/emr-spark-8.0.0 shape (the open issue 20261108T151103Z-emr-reopens-emr-runjobflow-now-refuses-valid-release-labels.md reports exactly that refusal, ValidationException on emr-spark-8.0.0, for the session checks that do NOT retry and therefore surface an error instead of a timeout) or the session is created and left parked in a pre-running state so GetSession never yields STARTED/IDLE/BUSY and TerminateSession never settles it to TERMINATED. Hypothesis: both halves are present — the refusal blocks cluster creation, and behind it the session state machine has no progression step. Neither 20261108T151103Z-emr-reopens-... nor 20261109T065007Z-emr-3-net-new-root-causes-modifyinstancefleet-accepts.md (StartSession/ListSessions 501 on a non-session-enabled cluster) carries these four checks.
Why: a client that creates a Spark Connect session and waits for it to be usable — the documented and only way to use the session data plane — waits forever; nothing it can poll ever reports STARTED, and after TerminateSession nothing ever reports TERMINATED, so neither a happy-path nor a teardown loop can terminate.
Fix: the RunJobFlow handler's release-label/SessionEnabled validation, plus the session state machine behind StartSession/GetSession/ListSessions/TerminateSession (session records and their state advancement in the EMR service state), and the newest-first ordering of ListSessions.
Traces: emr.session-lifecycle-start-get-terminate, emr.session-terminate-then-get-reflects-terminated-state, emr.list-sessions-ordering-newest-first, emr.e2e-studio-session-full-flow (all: "Check timed out after 60.0s").

Cause 2 of 3: a Spark step's data plane and the cluster's self-termination never happen (emr.emr-run-step-spark). The check creates an emr-6.15.0 cluster with KeepJobFlowAliveWhenNoSteps=false, submits one spark-submit step that writes the sum of squares 1..100 to S3, then polls (a) the cluster to RUNNING/WAITING/TERMINATED, (b) the step to any terminal StepState, (c) the job's S3 output object, (d) the cluster to TERMINATED, and finally asserts AddJobFlowSteps on the terminated cluster is refused.
AWS: a submitted step leaves PENDING, runs, and settles in a terminal StepState (Smithy: StepState = PENDING|CANCEL_PENDING|RUNNING|COMPLETED|CANCELLED|FAILED|INTERRUPTED); the Spark job writes its output object and the _SUCCESS marker to the S3 prefix the step names; a cluster whose Instances.KeepJobFlowAliveWhenNoSteps is false self-terminates once its last step is done, reaching ClusterState TERMINATED with reason ALL_STEPS_COMPLETED and disappearing from the live ListClusters states.
Emulator: the check never reaches any of its asserts — it is still polling at the 60 s cut-off, so at least one of "step leaves PENDING and settles terminal", "the step's S3 output exists" and "the cluster self-terminates" never happens. Hypothesis: the step is accepted and parked (the related open issues 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md and 20261105T074141Z-emr-events-sqs-a-step-submitted-to-a.md describe steps that never leave PENDING and clusters that never self-terminate), but neither of those issues carries this check.
Why: a client that submits a step and waits for it to finish — and for an auto-terminating cluster to shut down — gets no terminal status, no output object in S3 and a cluster that stays live forever, so every "submit and wait" workflow hangs instead of completing or failing.
Fix: the AddJobFlowSteps/DescribeStep step execution and state progression, the step's S3 output side effect, and the KeepJobFlowAliveWhenNoSteps=false self-termination path in the cluster state machine.
Traces: emr.emr-run-step-spark ("Check timed out after 60.0s").

Cause 3 of 3: a notebook execution never settles, so the ListNotebookExecutions Status filter cannot be exercised (emr.list-notebook-executions-ordering-and-status-filter). The check uploads one healthy notebook (print(1 + 1)) and one that raises, starts an execution of each on the same cluster, polls DescribeNotebookExecution for FINISHED/FAILED, then asserts Status=FINISHED and Status=FAILED partition the two and that two consecutive unfiltered listings are in the same order.
AWS: an execution walks START_PENDING -> STARTING -> RUNNING -> FINISHING -> FINISHED for the healthy notebook and -> FAILING -> FAILED for the raising one (Smithy: NotebookExecutionStatus = START_PENDING|STARTING|RUNNING|FINISHING|FINISHED|FAILING|FAILED|STOP_PENDING|STOPPING|STOPPED), and ListNotebookExecutions with Status= returns exactly the executions in that status.
Emulator: the healthy execution never reports FINISHED within the budget, so the check is killed while polling and never reaches the Status-filter assertions. Hypothesis: notebook executions are recorded but never executed (open issues 20261107T182201Z-emr-notebook-executions-never-run-a-healthy-notebook.md and 20261108T151103Z-emr-4-net-new-root-causes-every-job.md describe a healthy notebook ending FAILED with emulator-authored prose); neither carries this check.
Why: a client that starts a notebook execution and waits for FINISHED never gets it, and the Status filter it would then use to triage executions can never return a FINISHED one.
Fix: the StartNotebookExecution/DescribeNotebookExecution execution and status progression, and the Status filtering/ordering in the ListNotebookExecutions handler.
Traces: emr.list-notebook-executions-ordering-and-status-filter ("Check timed out after 60.0s").

## Evidence
Suite report (behavioral, service heading ### emr), 6 failing checks, each with the sole detail "Error: Check timed out after 60.0s" and no other diagnostic:
  [mismatch] emr.session-lifecycle-start-get-terminate
  [mismatch] emr.session-terminate-then-get-reflects-terminated-state
  [mismatch] emr.list-sessions-ordering-newest-first
  [mismatch] emr.e2e-studio-session-full-flow
  [mismatch] emr.emr-run-step-spark
  [mismatch] emr.list-notebook-executions-ordering-and-status-filter
None of the six is marked ALREADY FILED or OWNED by an open issue in this report.

Expected (AWS): each check runs to its assertions inside the budget because the state it polls advances on its own.
  - Session: StartSession -> GetSession State in {STARTED, IDLE, BUSY} (modeled SessionState; Session.StartedAt is "the date and time that the session entered the STARTED state"), TerminateSession -> GetSession State == TERMINATED with the session still readable, ListSessions newest-created first.
  - Step: DescribeStep reaches a terminal StepState, the step's S3 output object exists, DescribeCluster reaches TERMINATED (ALL_STEPS_COMPLETED) for KeepJobFlowAliveWhenNoSteps=false, and AddJobFlowSteps on it is then refused.
  - Notebook: DescribeNotebookExecution reaches FINISHED for the healthy notebook and FAILED for the raising one; ListNotebookExecutions(Status=...) partitions them.
Actual (emulator): no polled condition is ever satisfied; the runner kills each check at 60 s with the resource still in its initial/non-terminal state (or still inside the check's own RunJobFlow retry loop, which absorbs a RunJobFlow refusal of the SessionEnabled / emr-spark-8.0.0 shape). The emulator serves these reads in-memory, so the 60 s budget is ample — 103 checks in the same suite and the same budget complete; the cut-off is a state that never advances, not slow provisioning.

## Steps to Reproduce
Session lifecycle (cause 1):
1. RunJobFlow with ReleaseLabel=emr-spark-8.0.0, Applications=[{Name: Spark}], SessionEnabled=true, one MASTER instance group in a subnet, a service role and instance profile. Observe whether the call is accepted (a refusal is cause 1's first half).
2. Poll DescribeCluster until Status.State is WAITING or RUNNING.
3. StartSession(ClusterId, Name=...), then poll GetSession(ClusterId, SessionId) — it never reports State in {STARTED, IDLE, BUSY}.
4. TerminateSession, then poll GetSession — it never reports State == TERMINATED.
5. Start three sessions a few seconds apart and call ListSessions(ClusterId): the returned order is not newest-created first.

Spark step (cause 2):
1. RunJobFlow with ReleaseLabel=emr-6.15.0 and Instances.KeepJobFlowAliveWhenNoSteps=false; wait for RUNNING/WAITING.
2. AddJobFlowSteps with one spark-submit step whose script writes a result to s3://<bucket>/out/.
3. Poll DescribeStep: the step never settles in a terminal StepState; ListObjectsV2 on s3://<bucket>/out/ never shows the output object or _SUCCESS; DescribeCluster never reaches TERMINATED.

Notebook execution (cause 3):
1. Create a cluster with the notebook execution engine and wait for WAITING.
2. StartNotebookExecution for a notebook whose only cell is print(1 + 1), and a second one whose cell raises.
3. Poll DescribeNotebookExecution for both: the healthy one never reaches FINISHED, so ListNotebookExecutions(Status="FINISHED", ExecutionEngineId=<cluster>) never contains it.
