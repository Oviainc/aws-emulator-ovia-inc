Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states|run#25|AssertionError|assert elapsed_s >= 15.0, (
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# mwaa-serverless: a workflow run of a long-running task completes instantly — its own StartedOn/CompletedOn span ~1s for a 25-second sleep task, so RUNNING is never observable

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartWorkflowRun

## Description
ONE net-new root cause in the `mwaa-serverless` emulator, grounded in one failing behavioural check.

Model used: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json (sdkId "MWAA Serverless", awsJson1_0).

=== Cause 1 of 1: the run settlement does not model the duration of the work the definition declares, so a run reports a terminal state immediately and its own timestamps contradict the task it ran ===

AWS: Smithy: WorkflowRunDetail$RunState targets WorkflowRunStatus (STARTING|QUEUED|RUNNING|SUCCESS|FAILED|TIMEOUT|STOPPING|STOPPED). WorkflowRunDetail$StartedOn is "The timestamp when the workflow run started execution, in ISO 8601 date-time format"; WorkflowRunDetail$CompletedOn is "The timestamp when the workflow run completed execution ... This value is null if the run is not complete"; WorkflowRunDetail$Duration is "The total duration of the workflow run execution in seconds. This value is null if the run is not complete." Recorded behavior: for a DAG whose single BashOperator task is `sleep 25`, StartWorkflowRun answers a pre-run Status (STARTING or QUEUED), the run stays non-terminal while the task executes, and when it reaches SUCCESS its OWN reported CompletedOn - StartedOn is at least the task's runtime — the recording asserts >= 15 s for a 25-second task. In other words StartedOn/CompletedOn describe real execution of the declared work, and Duration is derived from the same window.

Emulator: the run is terminal essentially at once. The observed poll timeline is [(0.0 s, 'STARTING'), (2.0 s, 'SUCCESS')] — RUNNING is never seen — and the run's own RunDetail reports CompletedOn - StartedOn = 1.0 s for the `sleep 25` task. The reported timestamps are internally consistent with each other but contradict the definition the run executed: the settlement picks a fixed, work-independent window instead of one derived from the declared tasks.

Why: a client relying on AWS's behavior cannot observe a run in flight. Any code that polls GetWorkflowRun for a non-terminal RunState — a progress UI, a wait-until-done loop that wants to report RUNNING, a timeout guard, a StopWorkflowRun issued against a run still executing — has no window in which to act, so that whole branch is never exercised against the emulator and ships untried. Code that computes elapsed time or cost from StartedOn/CompletedOn (or reads Duration) gets a value with no relation to the work performed, so throughput and SLA logic validated against the emulator is wrong against AWS.

Fix: the run settlement in the service state (the workflow-execution module that advances a run through the WorkflowRunStatus lifecycle) — derive the run's non-terminal window and its StartedOn/CompletedOn/Duration from the execution plan built out of the definition's declared tasks, so a run with declared long-running work stays non-terminal for that duration, passes through RUNNING, and reports timestamps spanning it. Related but NOT covering this cause: open issue 20261029T185031Z-mwaa-serverless-six-behavioural-divergences-across-createworkflow-validation.md, whose Cause 5 asks for the definition reader to find `tasks` nested under the DAG-id key (so the plan is non-empty at all) and notes as a second effect that a stop must be issuable. This check is not in that issue's trace list and is a distinct requirement: even with a non-empty plan, a settlement that completes in 1 s still fails it, because the requirement here is that the reported StartedOn/CompletedOn window reflect the declared task's duration.

Traces: mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states.

## Evidence
Suite: simparity-behavioral-aws-v1. Service heading `mwaa-serverless`: 15/23 (65%), 8 failed; 7 of those are marked OWNED by 20261029T185031Z-mwaa-serverless-six-behavioural-divergences-across-createworkflow-validation.md and skipped. This is the one surviving failure.

mwaa-serverless.workflow-run-status-progresses-through-lifecycle-states (verdict: mismatch):
  DAG uploaded to DefinitionS3Location: one BashOperator task `only_task` with bash_command "sleep 25".
  StartWorkflowRun Status was an accepted pre-run value (STARTING/QUEUED) — that assertion passed.
  expected the run to reach SUCCESS with (CompletedOn - StartedOn) >= 15.0 s, i.e. a window consistent with a 25-second task
  actual   AssertionError: run completed in 1.0s per its own StartedOn/CompletedOn, too fast for the 25-second sleep task -- expected genuine execution, not an instant fake completion; timeline=[(0.0, 'STARTING'), (2.0, 'SUCCESS')]

The 1.0 s / 2.0 s figures are this run's values; the requirement is that the window track the declared work's duration, not any particular number. RunState went STARTING -> SUCCESS with no intermediate state observable at a 2-second poll interval (the check does not grade which intermediate states appear, only that the run is not instantly terminal).

## Steps to Reproduce
1. Create an S3 bucket and PutObject an Airflow YAML DAG at `dags/lifecycle.yaml` of the form:
     sim2real_dag:
       start_date: "2024-01-01"
       schedule: null
       tasks:
         only_task:
           operator: airflow.providers.standard.operators.bash.BashOperator
           bash_command: "sleep 25"
2. Create an IAM role whose trust policy names `airflow-serverless.amazonaws.com`.
3. CreateWorkflow(Name=<n>, DefinitionS3Location={Bucket,ObjectKey}, RoleArn=<role>, Tags={"created-by":"sim2real"}).
4. StartWorkflowRun(WorkflowArn) — expect Status in {STARTING, QUEUED}.
5. Poll GetWorkflowRun(WorkflowArn, RunId) every 2 s until RunDetail.RunState is terminal. Expect the run to be observable in a non-terminal state (and to pass through RUNNING) for roughly the task's 25-second runtime rather than being terminal on the first or second poll.
6. On the terminal read, assert RunState == "SUCCESS" and (RunDetail.CompletedOn - RunDetail.StartedOn).total_seconds() >= 15.0. Repeat with a shorter bash_command and confirm the reported window shrinks with it — the window must be derived from the declared work, not fixed.
