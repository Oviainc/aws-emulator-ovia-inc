Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.op-list-clusters-rejects-invalid-state-filter emr.list-notebook-executions-invalid-status-filter-rejected emr.op-run-job-flow-rejects-unknown-instance-profile emr.emr-serverless-spark-job-run-reaches-success emr.emr-serverless-failing-script-reports-failed-and-writes-no-output
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.op-list-clusters-rejects-invalid-state-filter|run#3|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: emr.list-notebook-executions-invalid-status-filter-rejected|run#3|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code!r}"
Issue-Key: emr.op-run-job-flow-rejects-unknown-instance-profile|_rejects_bogus_profile#6|AssertionError|raise AssertionError("expected the operation to be rejected, but it succeeded")
Issue-Key: emr.emr-serverless-spark-job-run-reaches-success|run#25|AssertionError|assert final == "SUCCESS", (
Issue-Key: emr.emr-serverless-failing-script-reports-failed-and-writes-no-output|run#28|AssertionError|assert fail_job.get("totalExecutionDurationSeconds", 0) > 0, (
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# EMR enum-filter refusals use unmodeled ValidationException, RunJobFlow accepts an unknown instance profile, and EMR Serverless job runs never execute (4 causes)

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
Four root causes in operations the emulator DOES serve: two misclassified client errors, one missing validation that makes the emulator overly permissive, and one data-plane gap in the EMR Serverless job-run state machine. All four are reached after a successful call, so they are fidelity divergences rather than scaffold gaps.

Cause 1 of 4: ListClusters refuses an invalid ClusterStates value with an unmodeled ValidationException instead of InvalidRequestException.
AWS: Smithy com.amazonaws.emr#ListClusters models exactly two errors — InvalidRequestException (HTTP 400, client) and InternalServerException (500). There is no ValidationException on this operation, so the only correct refusal for a ClusterStates entry outside ClusterStateList is 400 InvalidRequestException. The message is built from the request and is therefore a requirement too: "Cluster states [NOPE] are not valid." for ClusterStates=["NOPE"].
Emulator: refuses with error code 'ValidationException' (an error identity ListClusters does not model).
Why: a client that catches emr.exceptions.InvalidRequestException — the only client error in the SDK's error model for this operation — does not catch the emulator's refusal at all; it surfaces as a generic ClientError, so retry/validation branches keyed on the modeled error never run.
Fix: the refusal path in the operation's handler, services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_clusters.rs — raise the modeled InvalidRequestException with the request-derived message instead of the generic scaffold ValidationException.
Traces: emr.op-list-clusters-rejects-invalid-state-filter.

Cause 2 of 4: ListNotebookExecutions refuses an invalid Status value with ValidationException instead of InvalidRequestException.
AWS: Smithy com.amazonaws.emr#ListNotebookExecutions models InvalidRequestException (HTTP 400) and InternalServerError (500) only; Status targets the NotebookExecutionStatus enum, and a value outside it is refused 400 InvalidRequestException before any lookup. The HTTP status (400) is unchanged.
Emulator: error code 'ValidationException'.
Why: same as cause 1 — the modeled error identity is what SDK callers branch on, and the emulator answers one the model does not declare for this operation.
Fix: the enum-rejection path in services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_notebook_executions.rs.
Traces: emr.list-notebook-executions-invalid-status-filter-rejected.

Cause 3 of 4: RunJobFlow accepts a JobFlowRole naming an instance profile that does not exist.
AWS: RunJobFlow resolves JobFlowRole to a real EC2 instance profile before launching (Smithy: RunJobFlow.JobFlowRole, XmlString len 0..10280; the operation models no client error, so the refusal identity comes from recorded behavior). Recorded behavior: the call is refused with error code ValidationException and the request-derived message "Invalid InstanceProfile: <profile-name>.", and NO cluster is created (the rejection is atomic — the name never appears in ListClusters).
Emulator: RunJobFlow returns 200 with a JobFlowId for a profile name that was never created, so a cluster is fabricated for an unresolvable role.
Why: this is overly permissive in the most damaging direction — a client's misconfigured instance profile is silently accepted, the emulator reports a running cluster, and the failure that real EMR surfaces synchronously at launch time is never observed.
Fix: add the JobFlowRole resolution/validation step to services/service-com-amazonaws-emr-elasticmapreduce/src/ops/run_job_flow.rs (resolve the name against IAM instance-profile state and refuse before any cluster state is written).
Traces: emr.op-run-job-flow-rejects-unknown-instance-profile.

Cause 4 of 4: EMR Serverless job runs are recorded but never executed, so every run ends FAILED with no execution duration. (These checks live under the emr service token but drive the separate emr-serverless model, com.amazonaws.emrserverless#AwsToledoWebService, via ctx.client('emr-serverless').)
AWS: StartJobRun on a STARTED application runs the submitted Spark entry point and the run reaches state SUCCESS, with GetJobRun reporting a non-zero totalExecutionDurationSeconds; a run whose script raises reaches FAILED but still reports a non-zero totalExecutionDurationSeconds, because the driver started, worked and then raised. Recorded behavior only for the duration value's magnitude; the terminal states are the model's jobRunState values.
Emulator: every run goes to FAILED with stateDetails "This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.", and totalExecutionDurationSeconds is absent (None) even on the failure path.
Why: a client cannot observe a successful job run at all — success and failure both report FAILED — so job-orchestration logic that branches on jobRunState or measures run duration is unexercisable and any emulator-driven test of a happy path inverts.
Fix: the job-run state machine and its duration accounting in services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_job_run.rs and get_job_run.rs (a recorded-only run should still converge to SUCCESS and stamp a duration derived from its own start/end timestamps).
Traces: emr.emr-serverless-spark-job-run-reaches-success, emr.emr-serverless-failing-script-reports-failed-and-writes-no-output.

## Evidence
Cause 1 — expected code InvalidRequestException, HTTP 400, message "Cluster states [NOPE] are not valid."; actual AssertionError: "expected InvalidRequestException, got ValidationException" for emr.list_clusters(ClusterStates=["NOPE"]).
Cause 2 — expected code InvalidRequestException, HTTP 400; actual AssertionError: "expected InvalidRequestException, got 'ValidationException'" for emr.list_notebook_executions(Status="NOT_A_REAL_STATUS").
Cause 3 — expected code ValidationException with message "Invalid InstanceProfile: sim2real-behav-emr-no-such-profile." and no cluster created; actual AssertionError: "expected the operation to be rejected, but it succeeded" (RunJobFlow returned a JobFlowId).
Cause 4 — expected final jobRunState SUCCESS; actual AssertionError: "the Spark job run must reach SUCCESS, got 'FAILED' (stateDetails: 'This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.')". And on the failure path, expected non-zero totalExecutionDurationSeconds; actual AssertionError: "the FAILED run must report a non-zero totalExecutionDurationSeconds ... got None".

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. emr = client('emr'); emr.list_clusters(ClusterStates=['NOPE']) -> expect 400 InvalidRequestException "Cluster states [NOPE] are not valid."; observe ValidationException.
3. emr.list_notebook_executions(Status='NOT_A_REAL_STATUS') -> expect 400 InvalidRequestException; observe ValidationException.
4. Create only an EMR service role (no instance profile), then emr.run_job_flow(Name=..., ReleaseLabel='emr-6.15.0', Instances={...}, ServiceRole=<role>, JobFlowRole='no-such-profile') -> expect 400 ValidationException 'Invalid InstanceProfile: no-such-profile.' and nothing in ListClusters; observe 200 with a JobFlowId.
5. ems = client('emr-serverless'); create and start an application, StartJobRun with a trivial Spark entry point, poll GetJobRun -> expect SUCCESS with totalExecutionDurationSeconds > 0; observe FAILED with the emulator's stateDetails and no duration.
6. Re-replay the check ids in trace_ids.
