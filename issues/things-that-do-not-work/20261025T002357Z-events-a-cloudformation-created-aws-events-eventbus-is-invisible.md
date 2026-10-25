Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.deploy-cfn-bus-target-with-a-stack-created-execution-role-forwards-events-to-the-destination-bus events.deploy-cfn-event-bus-policy-admits-the-named-role-and-a-stack-update-removing-it-denies-again events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy events.deploy-eventbus-rule-target-cfn-then-put-events-delivers events.deploy-cfn-schedule-rule-fires-into-its-target-and-a-stack-update-disabling-it-stops-firing
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.deploy-cfn-bus-target-with-a-stack-created-execution-role-forwards-events-to-the-destination-bus|run#9|AssertionError|assert status == "CREATE_COMPLETE", f"stack create ended {status!r}: {_failure_reasons(cfn, stack)!r}"
Issue-Key: events.deploy-cfn-event-bus-policy-admits-the-named-role-and-a-stack-update-removing-it-denies-again|run#13|AssertionError|assert status == "CREATE_COMPLETE", f"stack create ended {status!r}: {_failure_reasons(cfn, stack)!r}"
Issue-Key: events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy|run#9|AssertionError|assert status == "CREATE_COMPLETE", f"stack create ended {status!r}: {_failure_reasons(cfn, stack)!r}"
Issue-Key: events.deploy-eventbus-rule-target-cfn-then-put-events-delivers|run#13|AssertionError|assert status == "CREATE_COMPLETE", f"stack {stack_name} ended in unexpected status {status}"
Issue-Key: events.deploy-cfn-schedule-rule-fires-into-its-target-and-a-stack-update-disabling-it-stops-firing|_receive#15|Timeout|for m in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5).get("Messages", []):
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# A CloudFormation-created AWS::Events::EventBus is invisible to the rest of its own stack, so every EventBridge stack rolls back; and a CFN-declared schedule rule never fires (2 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventBus

## Description
Cause 1 of 2 — resources that reference a stack-created event bus cannot resolve it.
AWS: within one stack, `AWS::Events::Rule` (EventBusName), `AWS::Events::EventBusPolicy` (EventBusName) and a rule target of type event bus are created after the `AWS::Events::EventBus` they depend on, and resolve its Ref/name to the bus that was just created. The stack reaches CREATE_COMPLETE.
Emulator: the dependent resource fails with `ResourceNotFoundException: Event bus <name> does not exist.` and the stack ends CREATE_FAILED / rolls back. The bus resource itself is reported created, but the name the stack hands to the next resource resolves to nothing — so either the bus is not registered in the EventBridge store under the name CFN uses, or the Ref/GetAtt the dependent resource is given is not that name.
Why: no EventBridge topology can be deployed by CloudFormation at all. Four separate stack shapes (bus + forwarding target, bus + bus policy, bus + rule with DeadLetterConfig, bus + rule + target) all fail at the same point, which is also the gate for the stack-update halves of those checks (adding a queue policy, removing a role from a bus policy).
Fix: the AWS::Events::EventBus provider — the name/physical-resource-id it registers and returns, and the EventBridge state lookup used by the AWS::Events::Rule / AWS::Events::EventBusPolicy providers.
Traces: events.deploy-cfn-bus-target-with-a-stack-created-execution-role-forwards-events-to-the-destination-bus, events.deploy-cfn-event-bus-policy-admits-the-named-role-and-a-stack-update-removing-it-denies-again, events.deploy-cfn-target-dead-letter-config-captures-undeliverable-events-until-a-stack-update-adds-the-queue-policy, events.deploy-eventbus-rule-target-cfn-then-put-events-delivers.

Cause 2 of 2 — a CFN-declared scheduled rule never fires into its target.
AWS: an `AWS::Events::Rule` with a ScheduleExpression on the default bus fires on its schedule into its target, and a stack update setting State: DISABLED stops it firing.
Emulator: the check's terminal read — the target's own effect — is never observed and the check is cut off at its 60 s cap, so the schedule never fires and the disable half can never be graded.
Why: the single most common serverless-cron pattern (CFN/SAM scheduled rule -> target) produces nothing; a client sees a healthy stack and a rule that is never invoked.
Fix: the EventBridge scheduler that evaluates rule ScheduleExpressions and invokes targets, plus the AWS::Events::Rule provider's State handling.
Traces: events.deploy-cfn-schedule-rule-fires-into-its-target-and-a-stack-update-disabling-it-stops-firing.

## Evidence
Cause 1 — stack outcomes reported by the replay:
- "stack create ended 'CREATE_FAILED': ['The following resource(s) failed to create: [Forward]. ResourceNotFoundException: Event bus sim2real-evb-b2bsrc-<id> does not exist.']"
- "stack create ended 'CREATE_FAILED': ['The following resource(s) failed to create: [Rule]. ResourceNotFoundException: Event bus sim2real-evb-cfnpol-<id> does not exist.']"
- "stack create ended 'CREATE_FAILED': ['The following resource(s) failed to create: [Rule]. ResourceNotFoundException: Event bus sim2real-evb-cfnbus-<id> does not exist.']"
- "stack sim2real-evb-stack-<id> ended in unexpected status CREATE_FAILED"
Expected: CREATE_COMPLETE, with the rule/policy bound to the bus created in the same stack.

Cause 2 — "Check timed out after 60.0s": the scheduled rule's target effect never appears, so the positive control for the disable half is never established. Expected: the target is invoked on the schedule, then stops once the stack update sets State: DISABLED.

## Steps to Reproduce
1. CreateStack with a template containing an AWS::Events::EventBus (Name: <n>) and an AWS::Events::Rule whose EventBusName is !Ref / !GetAtt of that bus plus one target -> stack ends CREATE_FAILED with "Event bus <n> does not exist."
2. Same with AWS::Events::EventBusPolicy instead of the rule -> same failure.
3. Same with a rule carrying Target.DeadLetterConfig -> same failure.
4. CreateStack with a default-bus AWS::Events::Rule carrying ScheduleExpression: rate(1 minute) and an SQS target -> the queue never receives a message; a stack update setting State: DISABLED cannot be graded.
