Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.glue-workflow-state-change-to-sqs-positive
Issue-Key: glue__sqs.glue-workflow-state-change-to-sqs-positive|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: cf7df418f843e0a7b878589d1abafe23db84fa21

# glue__sqs: an ON_DEMAND trigger that starts a workflow initiates no job run, so no "Glue Job State Change" event is ever published and the EventBridge->SQS target receives nothing

> _Note: this issue was identified against the code base at commit `cf7df418f843e0a7b878589d1abafe23db84fa21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTrigger

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: firing a workflow's ON_DEMAND start trigger records a workflow run but initiates none of the trigger's Actions, so no job run exists to change state, no "Glue Job State Change" event is published, and the workflow run can never complete ===
AWS: a workflow is driven by its starting trigger; `Trigger$Actions` are "The actions initiated by this trigger" and each `Action.JobName` is "The name of a job to be run" (Smithy: com.amazonaws.glue#Trigger, com.amazonaws.glue#Action). Starting the ON_DEMAND trigger of a workflow therefore (a) begins a workflow run, (b) starts a run of each job the trigger's actions name AS PART OF that workflow run, (c) emits the standard `aws.glue` EventBridge event with detail-type "Glue Job State Change" for each job-run state transition, whose `detail` carries `jobName`, `state` and `jobRunId`, so an EventBridge rule matching {"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":[J],"state":["SUCCEEDED"]}} with an SQS queue target delivers that event to the queue, and (d) brings the workflow run to `Status` COMPLETED with its graph's JOB node carrying the matching `JobDetails.JobRuns[].Id`, which `GetWorkflowRun(IncludeGraph=True)` reports. Glue publishes no distinct workflow-level detail-type — the workflow surfaces through its jobs' events, which is why the delivered `jobRunId` must appear among the workflow run's own job runs. Recorded behavior supplies only the delivery LATENCY (the check allows 480 s), never a fixed timing requirement.
Emulator: the check produced no API error and no assertion failure — the harness killed it: "Check timed out after 60.0s". Every call before the wait is implemented and answers 200 (s3.create_bucket/put_object, iam.create_role/attach_role_policy/put_role_policy, glue.create_job, glue.create_workflow, glue.create_trigger, events.put_rule/put_targets, sqs.create_queue/get_queue_attributes/set_queue_attributes), and glue StartTrigger is implemented (services/service-com-amazonaws-glue-awsglue/src/ops/start_trigger.rs answers 200 for an ON_DEMAND trigger), so the only wait whose budget exceeds the 60 s harness window and that can still be unsatisfied is `_drain_queue_until_match` (budget 480 s): the queue never receives a matching message. The reason is deterministic in the handler, not a hypothesis: on a fire it calls only `workflow.begin_run(...)` (start_trigger.rs lines 204-233) and initiates NO action — "Nothing here runs the jobs and crawlers a trigger's actions initiate" (start_trigger.rs lines 15-19), reading `trigger.actions.len()` only as a count. The identical defect is proven directly by the sibling check `glue.trigger-on-demand-lifecycle`, where `GetJobRuns` stays empty after a fire. With no job run there is no job-state transition, hence no "Glue Job State Change" event for EventBridge to match, hence nothing for the SQS target to receive; and the workflow run the fire did record cannot reach COMPLETED or carry any `JobDetails.JobRuns` in its graph. (Related, but NOT covering this check: 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md and 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md describe runs created by `StartJobRun` not executing and publishing no events; here no run is created at all, by a trigger fire rather than a client call. 20261026T234618Z-events-glue-with-glue-createtrigger-now-implemented-the.md covers the inbound EventBridge->EVENT-trigger ingress, the opposite direction. 20261026T234618Z-glue-starttrigger-stub.md claims StartTrigger answers 501, which no longer reproduces.)
Why: this is the canonical Glue eventing contract — orchestration, alerting and cost tooling all key off "Glue Job State Change" reaching a queue, a topic or a Lambda. A client that builds the documented chain sees every setup call succeed, `StartTrigger` return 200, and then absolute silence: no job run, no event, no error, and a workflow run that never terminates. There is no signal distinguishing "still running" from "nothing was ever dispatched", so retry and timeout logic built against AWS hangs, and the cross-check that the delivered `jobRunId` belongs to this workflow execution is wholly ungraded.
Fix: the glue StartTrigger handler and the workflow-run state it writes — initiate the trigger's `Actions` as job runs attached to the workflow run it begins, record each run on the run's graph JOB node (`JobDetails.JobRuns`) so `GetWorkflowRun(IncludeGraph=True)` reports it, publish an `aws.glue` / "Glue Job State Change" event carrying `jobName`, `state` and `jobRunId` on each job-run transition through the same event-publication path EventBridge rules match, and let the workflow run reach COMPLETED once its job runs are terminal.
Traces: glue__sqs.glue-workflow-state-change-to-sqs-positive; the same dispatch gap, shown directly by an assertion, in glue.trigger-on-demand-lifecycle (services/service-com-amazonaws-glue-awsglue/src/ops/start_trigger.rs lines 15-19 and 193-249).

## Evidence
glue__sqs.glue-workflow-state-change-to-sqs-positive [mismatch]

Actual (emulator): "Error: Check timed out after 60.0s" — no assertion failure and no API error; the check was killed while blocked in a bounded wait. Of its waits, only `_drain_queue_until_match` (480 s) and `_start_trigger_with_retry` (90 s) exceed the 60 s harness window, and StartTrigger is implemented and answers 200 for an ON_DEMAND trigger (start_trigger.rs), so the stall is the queue drain: no message matching detail.jobName == J ever arrives.

What the emulator demonstrably does, which pins the cause rather than leaving it to inference: start_trigger.rs fires an ON_DEMAND trigger by calling `workflow.begin_run(...)` ONLY (lines 204-233) and initiates none of the trigger's Actions — its own module doc states "Nothing here runs the jobs and crawlers a trigger's actions initiate" (lines 15-19). The sibling check glue.trigger-on-demand-lifecycle asserts this directly against the wire: after one start_trigger, "ON_DEMAND start_trigger should produce exactly one job run, got 0" from glue.get_job_runs. No job run => no job-state transition => no "Glue Job State Change" event => nothing for the EventBridge rule's SQS target.

Expected (AWS, per the model/docs cited in the description), after glue.start_trigger(Name=T) on an ON_DEMAND trigger with WorkflowName=W and Actions=[{'JobName': J}]:
- sqs.receive_message on the rule's target queue eventually returns a message whose JSON body has body['detail']['jobName'] == J, body['detail']['state'] == 'SUCCEEDED' and a non-empty body['detail']['jobRunId'] (source 'aws.glue', detail-type 'Glue Job State Change').
- glue.get_workflow_runs(Name=W) reports one run, and glue.get_workflow_run(Name=W, RunId=<it>, IncludeGraph=True)['Run']['Status'] reaches 'COMPLETED'.
- that run's Graph contains a node with Type 'JOB' whose JobDetails.JobRuns includes an Id equal to the delivered body['detail']['jobRunId'].
Emulator: the queue stays empty, so none of these is reached.

Generated bucket/role/job/workflow/trigger/rule/queue names, the account id, the delivered run ids and the 60 s harness budget are per-run values, not requirements.

## Steps to Reproduce
1. s3.create_bucket(Bucket=B); s3.put_object(Bucket=B, Key='scripts/job.py', Body=b"print('ok')\n").
2. iam.create_role(RoleName=R, AssumeRolePolicyDocument=<trust glue.amazonaws.com>); attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole; put_role_policy granting s3:GetObject/PutObject/ListBucket on B.
3. glue.create_job(Name=J, Role=<R arn>, Command={'Name':'pythonshell','ScriptLocation':'s3://B/scripts/job.py','PythonVersion':'3.9'}, GlueVersion='3.0', MaxCapacity=0.0625) -> 200.
4. glue.create_workflow(Name=W) -> 200; glue.create_trigger(Name=T, WorkflowName=W, Type='ON_DEMAND', Actions=[{'JobName': J}]) -> 200.
5. events.put_rule(Name=r, State='ENABLED', EventPattern='{"source":["aws.glue"],"detail-type":["Glue Job State Change"],"detail":{"jobName":["J"],"state":["SUCCEEDED"]}}'); sqs.create_queue(QueueName=q); sqs.set_queue_attributes(Policy allowing events.amazonaws.com sqs:SendMessage from the rule ARN); events.put_targets(Rule=r, Targets=[{'Id':'t','Arn':<queue arn>}]).
6. glue.start_trigger(Name=T) -> 200.
7. Poll glue.get_job_runs(JobName=J).
   -> AWS: one JobRun appears, driven by the workflow run.
   -> Emulator: {'JobRuns': []} (same as glue.trigger-on-demand-lifecycle). This is the root cause.
8. Poll sqs.receive_message(QueueUrl=<q>, MaxNumberOfMessages=10, WaitTimeSeconds=10) for up to 480 s.
   -> AWS: a message whose body['detail'] has jobName J, state 'SUCCEEDED' and a jobRunId.
   -> Emulator: no message ever arrives; the harness kills the check at 60 s.
9. (What must pass after the fix) glue.get_workflow_runs(Name=W) reports one run; glue.get_workflow_run(Name=W, RunId=<it>, IncludeGraph=True)['Run']['Status'] == 'COMPLETED'; and the delivered body['detail']['jobRunId'] is among [jr['Id'] for node in Run['Graph']['Nodes'] if node['Type']=='JOB' for jr in node['JobDetails']['JobRuns']].
10. Replay glue__sqs.glue-workflow-state-change-to-sqs-positive to confirm the fix.
