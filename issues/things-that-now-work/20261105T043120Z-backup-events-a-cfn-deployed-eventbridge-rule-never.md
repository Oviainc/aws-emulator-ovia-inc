Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__events.cfn-deploy-eventbridge-rule-without-target-permission-rejected
Issue-Key: backup__events.cfn-deploy-eventbridge-rule-without-target-permission-rejected|_poll_until#13|Timeout|last = probe()
Issue-Commit: 07e6c5615b7c4705e5fb2d8839b0f757e6d16be1

# backup__events: a CFN-deployed EventBridge rule never invokes its permitted Lambda target, and the un-permitted sibling target's event is never dead-lettered

> _Note: this issue was identified against the code base at commit `07e6c5615b7c4705e5fb2d8839b0f757e6d16be1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Two root causes, both observed by the one check whose replay key no open issue carries (`backup__events.cfn-deploy-eventbridge-rule-without-target-permission-rejected`). The stack declares two AWS::Events::Rule resources with the SAME event pattern (source `sim2real.backup`, detail-type `Backup Job State Change`), each with one AWS::Lambda::Function target and a per-target DeadLetterConfig; only the first function carries an AWS::Lambda::Permission granting Principal events.amazonaws.com `lambda:InvokeFunction` with SourceArn = the rule ARN. One PutEvents entry is then published and the check observes (1) the permitted function's work product in a success queue and (2) the un-permitted target's event in that target's dead-letter queue.

Cause 1 of 2: the matching rule's PERMITTED Lambda target is never invoked, so the function never runs and the success queue it writes to stays empty for the whole poll window.
Failing checks: backup__events.cfn-deploy-eventbridge-rule-without-target-permission-rejected.
AWS: PutEvents accepts the entry (PutEventsResponse: FailedEntryCount 0, one Entries[].EventId) and EventBridge then matches it against every rule on the bus and delivers the complete event envelope to each target of each matching ENABLED rule. For a Lambda target whose resource policy grants events.amazonaws.com `lambda:InvokeFunction` on the rule ARN, EventBridge invokes the function asynchronously, normally within seconds, and the handler's own SQS SendMessage then puts the envelope on the success queue. Smithy (eventbridge): PutEvents -> PutEventsResponse{FailedEntryCount, Entries}; PutTargets.Targets[].Arn/RoleArn/DeadLetterConfig/RetryPolicy are modelled, the delivery itself is recorded behavior only.
Emulator: PutEvents is accepted and nothing is ever delivered. The success queue never receives a message; the check exhausts its poll budget and the suite reports `Check timed out after 60.0s`.
Why: a client whose pipeline is PutEvents -> rule -> Lambda sees its events accepted and silently discarded — no invocation, no error, no DLQ entry — so a working wiring is indistinguishable from a broken one.
Fix: the rule-target delivery path of the events crate (pattern match -> per-target dispatch -> Lambda Invoke, honouring the target function's resource-based policy so an ungranted target is skipped while a granted sibling is invoked), plus the state that records CFN-created AWS::Events::Rule targets and AWS::Lambda::Permission statements so the delivery path can see them.

Cause 2 of 2: a target that cannot be invoked produces no dead-letter message — the target's DeadLetterConfig queue is never written.
Failing checks: backup__events.cfn-deploy-eventbridge-rule-without-target-permission-rejected.
AWS: when EventBridge cannot deliver to a target (here: the Lambda function has NO resource-policy grant for events.amazonaws.com, so Invoke is refused with AccessDeniedException) and the target carries a DeadLetterConfig, the event is written to that SQS queue after the target's RetryPolicy is exhausted (this target sets MaximumRetryAttempts 0, so immediately). The dead-letter message body is the event envelope, so it still correlates to the triggering event (here by `detail.backupJobId`). Smithy (eventbridge) Target.DeadLetterConfig: "The DeadLetterConfig that defines the target queue to send dead-letter queue events to." The symmetric requirement also holds: the sibling target that CAN be invoked must NOT dead-letter, so its own DLQ stays empty.
Emulator: both dead-letter queues stay empty for the whole window. Because nothing is delivered and nothing is dead-lettered, the permitted and the un-permitted target behave identically, and the permission boundary the check exists to verify cannot be observed at all.
Why: a client relying on DeadLetterConfig to capture undeliverable events loses them silently; an operator debugging a missing invocation finds neither the invocation nor the dead letter that AWS would have left behind.
Fix: the target-failure path in the events crate — classify the failed target invocation (no permission) and enqueue the envelope to the target's DeadLetterConfig ARN after RetryPolicy exhaustion — and the target state that persists DeadLetterConfig/RetryPolicy from both PutTargets and CloudFormation-created rules.

Related OPEN issues cover neighbouring backup/EventBridge checks but do NOT carry this check's replay key: 20261029T175130Z-backup-events-no-backup-copy-restore-recovery-point.md, 20261101T225522Z-backup-events-the-cfn-deployed-backup-plan-eventbridge.md, 20261102T091831Z-backup-events-no-aws-backup-state-change-event.md, and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (the Lambda side of the same path).

Traces: backup__events.cfn-deploy-eventbridge-rule-without-target-permission-rejected — times out while polling the success queue, having already shown both DLQs empty.

## Evidence
Expected (AWS): after the stack reaches CREATE_COMPLETE and both functions report State=Active, events.put_events(Entries=[{Source:"sim2real.backup", DetailType:"Backup Job State Change", Detail:{backupJobId:<marker>, state:"COMPLETED"}}]) returns FailedEntryCount 0; within seconds the permitted target's function runs and SendMessage puts a body containing <marker> on the success queue; the un-permitted target's invocation is refused (AccessDeniedException on lambda:InvokeFunction for Principal events.amazonaws.com) and, with MaximumRetryAttempts 0, the envelope (still containing <marker>) is written to that target's DeadLetterConfig queue; the permitted target's own DLQ receives nothing.
Actual (emulator): put_events is accepted, then nothing happens. receive_message on the success queue returns no Messages for the entire poll, receive_message on the un-permitted target's DLQ returns no Messages, and the check ends `Check timed out after 60.0s`. No error is surfaced by CreateStack, PutTargets or PutEvents — delivery and dead-lettering are both absent, so the permitted and un-permitted targets are indistinguishable.

## Steps to Reproduce
1. cloudformation.create_stack with a template containing: an SQS success queue, two SQS dead-letter queues, an IAM role for Lambda, two python3.12 AWS::Lambda::Function resources (the first sends the received event to the success queue), an AWS::Lambda::Permission for the FIRST function only (Action lambda:InvokeFunction, Principal events.amazonaws.com, SourceArn = Fn::GetAtt GoodRule.Arn), and two AWS::Events::Rule resources with EventPattern {"source":["sim2real.backup"],"detail-type":["Backup Job State Change"]} — rule A targeting the permitted function with DeadLetterConfig=DlqGood, rule B targeting the un-permitted function with RetryPolicy{MaximumRetryAttempts:0} and DeadLetterConfig=DlqBad. Wait for CREATE_COMPLETE.
2. Poll lambda.get_function until both functions report Configuration.State == "Active".
3. events.put_events(Entries=[{Source:"sim2real.backup", DetailType:"Backup Job State Change", Detail: json({"backupJobId": "<marker>", "state": "COMPLETED"})}]).
4. sqs.receive_message on the success queue URL (stack output) — AWS yields a body containing <marker>; the emulator yields nothing.
5. sqs.receive_message on the un-permitted target's DLQ URL — AWS yields the dead-lettered envelope containing <marker>; the emulator yields nothing.
6. sqs.receive_message on the permitted target's DLQ URL — empty on both (control).
