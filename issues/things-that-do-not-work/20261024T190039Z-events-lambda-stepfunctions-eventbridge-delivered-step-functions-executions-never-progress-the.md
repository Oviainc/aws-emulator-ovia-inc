Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id events__lambda__stepfunctions.refarch-sfn-nonmatching-event-source-starts-no-execution
Issue-Key: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id|run#32|AssertionError|assert final["status"] == "SUCCEEDED", (
Issue-Key: events__lambda__stepfunctions.refarch-sfn-nonmatching-event-source-starts-no-execution|run#20|Timeout|time.sleep(5)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# EventBridge-delivered Step Functions executions never progress: the Lambda-polling Task state is not executed, and the rule-to-state-machine positive control never completes within the check budget (2 causes)

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Two causes on the EventBridge-rule -> state-machine -> Lambda reference-architecture edge.

Cause 1 of 2: an execution started by an EventBridge rule target fails instead of running its job-poller definition.
AWS: recorded behavior only — a matched event is delivered to the state-machine target, the execution's input is the EventBridge envelope, and the definition's Task states (arn:aws:states:::lambda:invoke of a submit function and a check function, with a Wait/Choice poll loop) run to a SUCCEEDED execution whose output carries the envelope's id as the job id.
Emulator: the execution reaches a terminal FAILED with error='States.Runtime' and cause="state 'SubmitJob' is of type 'Task', which this emulator does not execute, so nothing was run for it". Delivery happened; execution did not.
Why: the entire event-driven job-poller pattern is unusable — the envelope-to-job-id correlation a client reads from the execution output never exists, and the failure it does see names an error no AWS workflow produces for this definition.
Fix: the Step Functions interpreter's Task-state dispatch (lambda:invoke resource) and its Wait/Choice poll loop.
Traces: events__lambda__stepfunctions.refarch-sfn-eventbridge-envelope-id-becomes-the-job-id

Cause 2 of 2: the pattern-filtering negative cannot be reached because its positive control never lands — the check exhausts its 60 s budget with no completed round.
AWS: recorded behavior only — PutEvents on a source matching the rule's pattern produces a new execution of the target state machine within seconds, which is the precondition for asserting that an event on a NON-matching source produces none.
Emulator: the check times out at 60.0 s. Hypothesis: the barrier execution the check runs before arming the rule never leaves RUNNING/FAILED-fast for the same Task-dispatch reason as Cause 1, and/or no execution appears for a matching event, so the arming loop never observes a delivered execution.
Why: the half of EventBridge fidelity that matters most — that a rule FILTERS rather than forwarding everything — cannot be demonstrated at all, so an over-permissive router would go unnoticed.
Fix: EventBridge rule-to-state-machine target delivery, plus the interpreter path from Cause 1.
Traces: events__lambda__stepfunctions.refarch-sfn-nonmatching-event-source-starts-no-execution

Related open issues that do NOT carry these checks' replay keys: 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md, 20261023T140640Z-cloudfront-events-...md (EventBridge never delivers a matched event to any target).

## Evidence
Cause 1: AssertionError: the delivered execution should run the poller to completion; got 'FAILED' error='States.Runtime' cause="state 'SubmitJob' is of type 'Task', which this emulator does not execute, so nothing was run for it"

Cause 2: Error: Check timed out after 60.0s (the check republishes a matching event every 6 s and polls ListExecutions for a new execution; no round ever observed one, so the negative window never opened).

## Steps to Reproduce
Cause 1: create two Lambda functions (submit/check), a state machine whose definition is a Task -> Wait -> Task -> Choice poller over lambda:invoke, and an EventBridge rule matching a custom source with the state machine as target. PutEvents a matching event, then poll DescribeExecution on the created execution: expected SUCCEEDED with the envelope id as the job id; emulator returns FAILED / States.Runtime.
Cause 2: with the same estate, publish events on the matching source in a loop and poll ListExecutions for a new execution name; expected one within seconds, after which events on a non-matching source must produce none. Emulator never surfaces the positive-control execution and the check hits the 60 s cap.
