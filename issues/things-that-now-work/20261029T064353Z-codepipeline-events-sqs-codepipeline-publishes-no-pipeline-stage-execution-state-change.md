Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage
Issue-Key: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage|run#32|TimeoutError|raise TimeoutError("the graded pipeline's execution state change events: the run budget ended first; not graded")
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# CodePipeline publishes no pipeline/stage execution state-change event to EventBridge even when the execution Succeeds, so a rule's SQS target receives nothing

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
ONE net-new root cause (Cause 1 of 1). A fixer must close it before submitting.

Related but NOT covering this failure: the open issue 20261028T112857Z-codepipeline-events-sqs-codepipeline-execution-stage-state-change-events-still-never.md describes the same shape but carries this check's PREVIOUS replay key (…|_receive#33|Timeout|resp = sqs.receive_message(...)), and the older 20261022T173520Z-codepipeline-events-sqs-codepipeline-stage-action-state-change-events-are-never.md holds an even earlier key. This replay marks the check neither ALREADY FILED nor OWNED, so no open issue carries its current key, and the evidence has changed decisively: BOTH pipelines now run to a terminal 'Succeeded' status (the check asserts that before it starts collecting), so the execution engine does progress and the remaining gap is purely event emission/delivery.

Cause 1 of 1: no 'CodePipeline Pipeline Execution State Change' or 'CodePipeline Stage Execution State Change' event is published for an execution, so an EventBridge rule matching them with an SQS target delivers no message.

AWS: recorded behavior only — the codepipeline Smithy model (com.amazonaws.codepipeline#CodePipeline_20150709; StartPipelineExecution -> pipelineExecutionId, GetPipelineState -> stageStates) models no event emission. Per the EventBridge/CodePipeline references the check cites, while an execution runs CodePipeline emits one event per state transition with source 'aws.codepipeline', detail-type 'CodePipeline Pipeline Execution State Change' / 'CodePipeline Stage Execution State Change' / 'CodePipeline Action Execution State Change', resources == [the pipeline ARN arn:aws:codepipeline:<region>:<account>:<pipelineName>], the caller's account and region on the envelope, and a detail carrying pipeline (name), execution-id, stage (for stage/action events) and state (STARTED, SUCCEEDED, FAILED, …). A rule whose EventPattern matches those and whose target is an SQS queue delivers one message per matched event, readable by ReceiveMessage; a rule scoped to detail.pipeline == [one name] delivers nothing for any other pipeline.
Emulator: the check creates the queue and its queue policy for events.amazonaws.com, puts the rule (pattern on source 'aws.codepipeline', the two detail-types and detail.pipeline == [the graded pipeline]), adds the queue as the target, creates a graded pipeline and an unmatched twin, waits for both executions to reach 'Succeeded' (which they now DO), and then polls ReceiveMessage for the six required (kind, stage, state) combinations — pipeline STARTED/SUCCEEDED and Source/Deploy stage STARTED/SUCCEEDED. Not one message of any kind arrives for the whole collection window, and the check ends with 'TimeoutError: the graded pipeline's execution state change events: the run budget ended first; not graded'. Hypothesis (neither the model nor a recording states the mechanism): the execution state machine publishes nothing to the event bus, so the rule has nothing to match; whether the rule-to-SQS-target delivery path also fails cannot be observed until emission exists.
Why: every event-driven integration on CodePipeline is dead. Notification fan-out, approval alerting, deployment dashboards, cross-account stage chaining and audit pipelines all wait for ever with no message and no error, even though the execution completed successfully — a client cannot detect that a pipeline started, advanced a stage or finished except by polling, and the ordering/attribution guarantees (execution-id, pipeline ARN in resources, account, region, Source SUCCEEDED before Deploy STARTED) are unobservable.
Fix: event emission from the codepipeline crate's execution state machine onto the EventBridge bus (the 'aws.codepipeline' source, the pipeline/stage/action detail-types and their detail + envelope serialization: resources, account, region, time), and the EventBridge rule-match -> SQS-target delivery path that turns a matched event into a queue message.
Traces: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage.

## Evidence
Verbatim from the replay report:
  #### [error] codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage
  Error: File "check_codepipeline__events__sqs_pipeline_execution_state_change_events_reach_the_queue_for_each_stage.py", line 119, in run
  raise TimeoutError("the graded pipeline's execution state change events: the run budget ended first; not graded")
  TimeoutError: the graded pipeline's execution state change events: the run budget ended first; not graded

The raise is inside the collection loop, which runs only AFTER the check's assertion that each execution reached 'Succeeded' — so both pipelines completed and the emulator still published nothing.
Expected: ReceiveMessage on the rule's target queue yields, for the graded pipeline, at least the set {(pipeline, STARTED), (pipeline, SUCCEEDED), (stage Source, STARTED), (stage Source, SUCCEEDED), (stage Deploy, STARTED), (stage Deploy, SUCCEEDED)}, each envelope with source 'aws.codepipeline', the matching detail-type, resources == [the pipeline's ARN], the caller's account and region, and detail.execution-id == the started execution id; and nothing from the unmatched twin pipeline.
Actual: the queue stays empty — 'collected' never acquires a single key — so none of the attribution or ordering assertions is ever reached.
Per-service pass rate for codepipeline__events__sqs in this run: 0/1 (0%), 1 failed.

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), eb = client('events'), sqs = client('sqs'), s3 = client('s3'), iam = client('iam'), sts = client('sts').
1. sqs.create_queue; read its QueueArn and set a queue policy allowing events.amazonaws.com to sqs:SendMessage on it for the rule ARN.
2. eb.put_rule(Name=R, State='ENABLED', EventPattern=json.dumps({'source': ['aws.codepipeline'], 'detail-type': ['CodePipeline Pipeline Execution State Change', 'CodePipeline Stage Execution State Change'], 'detail': {'pipeline': [P]}})) and eb.put_targets(Rule=R, Targets=[{'Id': 'queue', 'Arn': <queue arn>}]).
3. Create an artifact bucket with a 'source.zip' object and a pipeline role trusted by codepipeline.amazonaws.com; cp.create_pipeline for P (S3 Source -> S3 Deploy) and for an unmatched twin pipeline.
4. Let the execution of P run (or cp.start_pipeline_execution(name=P)) and poll cp.get_pipeline_execution until it reports 'Succeeded'.
5. Poll sqs.receive_message(QueueUrl=..., MaxNumberOfMessages=10) -> expect at least one message per pipeline- and stage-level state transition of P, each body carrying source 'aws.codepipeline', the matching detail-type, resources == ['arn:aws:codepipeline:<region>:<account>:'+P], detail.execution-id == the execution id, and no message for the twin pipeline; observe an empty queue indefinitely.
6. Re-replay codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage.
