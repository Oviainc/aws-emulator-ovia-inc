Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage
Issue-Key: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage|_poll#22|Timeout|time.sleep(interval_s)
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodePipeline stage/action state-change events are never delivered to an EventBridge rule's SQS target

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution

## Description
One root cause. Cause 1 of 1: no pipeline execution/stage state-change events are emitted to EventBridge, so an EventBridge rule targeting an SQS queue receives nothing.
AWS: while an execution runs, CodePipeline emits CloudWatch Events/EventBridge events with source "aws.codepipeline" and detail-type "CodePipeline Pipeline Execution State Change" / "CodePipeline Stage Execution State Change" / "CodePipeline Action Execution State Change", one per state transition of the execution and of each stage. An EventBridge rule matching those events and targeting an SQS queue delivers one message per matched event, which the client reads with ReceiveMessage.
Emulator: the check creates the rule and the SQS target, starts an execution, and then waits for the per-stage messages; it never receives them and hits the harness ceiling with "Error: Check timed out after 60.0s". This is consistent with — and most likely the same underlying gap as — the codepipeline execution engine never advancing an execution past InProgress (filed separately against service codepipeline): with no state transitions there is nothing to publish. The event-emission path must be implemented in addition to the engine, since no event of any kind (not even the initial STARTED transition) is observed.
Why: every event-driven integration on CodePipeline is dead — notification fan-out, approval alerting, cross-account chaining and audit pipelines all wait forever with no error and no message, rather than observing the execution's progress.
Fix: event emission from the codepipeline execution state machine into EventBridge (the aws.codepipeline event source and its detail-type/detail serialization), plus the EventBridge rule-match-to-SQS-target delivery path.
Traces: codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage.

## Evidence
Expected: one EventBridge event per pipeline/stage state transition (source "aws.codepipeline", detail-type "CodePipeline Pipeline Execution State Change" / "CodePipeline Stage Execution State Change") delivered as an SQS message to the rule's queue target, readable via ReceiveMessage. Actual: no message ever arrives; the check fails with "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Boot the emulator and replay codepipeline__events__sqs.pipeline-execution-state-change-events-reach-the-queue-for-each-stage.
2. Create an SQS queue, an EventBridge rule with an event pattern on source "aws.codepipeline", and add the queue as the rule's target.
3. Create a multi-stage pipeline and call StartPipelineExecution.
4. Poll ReceiveMessage on the queue — no pipeline or stage execution state-change event is ever delivered.
