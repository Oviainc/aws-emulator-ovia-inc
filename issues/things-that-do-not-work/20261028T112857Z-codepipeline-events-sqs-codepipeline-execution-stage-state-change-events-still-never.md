Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage
Issue-Key: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage|_receive#33|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Commit: aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6

# CodePipeline execution/stage state-change events still never reach an EventBridge rule's SQS target

> _Note: this issue was identified against the code base at commit `aeaf03eb4bcbb7a6c4e9492334e38170d85c4ec6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
One root cause. Cause 1 of 1: no CodePipeline pipeline/stage execution state-change event is published to EventBridge, so a rule whose target is an SQS queue receives nothing.

This check is NOT marked ALREADY FILED or OWNED in this replay, so no open issue carries its current replay key. Named for context, but NOT covering it: the open issue 20261022T173520Z-codepipeline-events-sqs-codepipeline-stage-action-state-change-events-are-never.md describes the same shape against an older commit and holds the check's previous key; it was filed from the simparity-behavioral-aws-v1 corpus and the failure persists in this run of simparity-behavioral-ga-prod-v1.

AWS: recorded behavior only (the codepipeline Smithy model does not describe event emission). While an execution runs, CodePipeline emits events with source 'aws.codepipeline' and detail-type 'CodePipeline Pipeline Execution State Change', 'CodePipeline Stage Execution State Change' and 'CodePipeline Action Execution State Change' — one per state transition of the execution, of each stage and of each action, the detail carrying the pipeline name, the execution id, the stage/action name and the new state. An EventBridge rule whose pattern matches those events and whose target is an SQS queue delivers one message per matched event, readable with ReceiveMessage.
Emulator: the check creates the queue, the rule and the target, starts an execution, and then polls ReceiveMessage for the per-stage messages. Nothing ever arrives — not even the initial STARTED transition — and the check stops at the suite's 60 s per-check ceiling ('Error: Check timed out after 60.0s'). Hypothesis (neither a model nor a recording states the mechanism): no event is published from the execution state machine at all, so the rule has nothing to match; the codepipeline execution-plane stalls filed separately in this sweep would also leave later transitions unpublished even once emission exists, so both must be fixed for the per-stage messages to appear.
Why: every event-driven integration on CodePipeline is dead — notification fan-out, approval alerting, cross-account stage chaining and audit pipelines all wait forever with no message and no error, rather than observing the execution's progress. Clients cannot even detect that the pipeline started.
Fix: event emission from the codepipeline crate's execution state machine into EventBridge (the 'aws.codepipeline' source with the three detail-types and their detail serialization), plus the EventBridge rule-match-to-SQS-target delivery path.
Traces: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage.

## Evidence
Quoted verbatim from the replay report:
  #### [mismatch] codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage
  Error: Check timed out after 60.0s
Expected: one EventBridge event per pipeline/stage state transition (source 'aws.codepipeline', detail-type 'CodePipeline Pipeline Execution State Change' / 'CodePipeline Stage Execution State Change'), each delivered as an SQS message to the rule's queue target and readable via ReceiveMessage. Actual: no message of any kind arrives. Per-service pass rate for codepipeline__events__sqs in this run: 0/1 (0%).

## Steps to Reproduce
Boot the emulator and point botocore at it; cp = client('codepipeline'), events = client('events'), sqs = client('sqs'), s3 = client('s3'), iam = client('iam').
1. sqs.create_queue, then events.put_rule with an event pattern on {'source': ['aws.codepipeline']} and events.put_targets adding the queue as the rule's target (with a queue policy allowing events.amazonaws.com to send).
2. Create an artifact bucket with a source object, a pipeline role, and a multi-stage pipeline.
3. cp.start_pipeline_execution(name=P).
4. Poll sqs.receive_message on the queue -> expect at least one message per pipeline and stage state transition, each body carrying source 'aws.codepipeline' and the matching detail-type; observe no message ever delivered.
5. Re-replay codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage.
