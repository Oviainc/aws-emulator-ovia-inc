Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.cdk-lambda-managed-log-group-flag-update-collides-with-service-created-group-rejected cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create cloudformation__lambda__logs.cfn-logs-subscription-filter-reads-back-and-forwards-a-matching-event-to-lambda
Issue-Key: cloudformation__lambda__logs.cdk-lambda-managed-log-group-flag-update-collides-with-service-created-group-rejected|run#39|AssertionError|assert described["Status"] == "FAILED", (
Issue-Key: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources|run#9|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create|_poll#25|AssertionError|raise AssertionError(f"{what} not reached within {timeout_s}s; last {last!r}")
Issue-Key: cloudformation__lambda__logs.cfn-logs-subscription-filter-reads-back-and-forwards-a-matching-event-to-lambda|run#13|AssertionError|assert physical_id == filter_name, (
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# cloudformation__lambda__logs: 4 net-new causes — change set does not fail on a pre-existing log group, a FAILED create leaves ROLLBACK_FAILED, a subscription filter never forwards, and its physical id is generated instead of the declared FilterName

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 4: a change set that would create a log group which already exists outside the stack is not failed.
AWS: CreateChangeSet for a template whose AWS::Logs::LogGroup name already exists outside the stack ends with Status 'FAILED' and a StatusReason naming the collision, so the deploy never starts. Recorded behavior: the change set reports FAILED.
Emulator: the change set reaches 'CREATE_COMPLETE' even though '/aws/lambda/<function>-<suffix>' already exists outside the stack.
Why: a client's guard against adopting a service-created log group disappears; the deploy proceeds and then fails mid-flight.
Fix: change-set evaluation for AWS::Logs::LogGroup in the CloudFormation crate.
Traces: cloudformation__lambda__logs.cdk-lambda-managed-log-group-flag-update-collides-with-service-created-group-rejected.

Cause 2 of 4: a custom resource whose Create answers FAILED leaves the stack in ROLLBACK_FAILED.
AWS: when a Create fails, CloudFormation rolls the stack back to ROLLBACK_COMPLETE and the resources it created are removed; the failure Reason is reported verbatim on the resource row.
Emulator: the stack ends 'ROLLBACK_FAILED' instead of ROLLBACK_COMPLETE, so the stack cannot be deleted or retried without manual intervention.
Why: a client's failed deploy is left in a state that blocks every subsequent operation on the stack.
Fix: the rollback path in the CloudFormation crate (a resource that failed to create must not fail its own rollback).
Traces: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources.

Cause 3 of 4: a CloudFormation-created subscription filter never delivers to its Lambda destination.
AWS: once the stack is CREATE_COMPLETE, events put into the source log group are forwarded to the destination function and the invocation is observable in the function's own log group.
Emulator: the positive control's invocation never happens — 'positive control's invocation not reached within 120.0s; last []'.
Why: the subscription-filter pipeline a client deploys silently delivers nothing.
Fix: subscription-filter delivery to a Lambda destination in the CloudWatch Logs crate as wired by the AWS::Logs::SubscriptionFilter handler.
Traces: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create.

Cause 4 of 4: the subscription filter's physical resource id is CloudFormation-generated instead of the declared FilterName.
AWS: AWS::Logs::SubscriptionFilter's physical id is the FilterName the template declares, so DescribeStackResource reports exactly that name and the filter can be addressed by it.
Emulator: the physical id is '<stack>-SubFilter-<random>' while the template declared '<filter name>', i.e. the handler generates a name even though one was given.
Why: a client cannot address or clean up the filter by the name it declared, and Ref/GetAtt consumers carry the wrong value.
Fix: the AWS::Logs::SubscriptionFilter handler's physical-id derivation in the CloudFormation crate.
Traces: cloudformation__lambda__logs.cfn-logs-subscription-filter-reads-back-and-forwards-a-matching-event-to-lambda.

## Evidence
Cause 1 — 'AssertionError: CreateChangeSet for the flag-enabled template reached status 'CREATE_COMPLETE', expected 'FAILED' — the log group '/aws/lambda/...' already exists outside the stack'.
Cause 2 — 'AssertionError: a Create that answers FAILED must roll the stack back to ROLLBACK_COMPLETE, got 'ROLLBACK_FAILED''.
Cause 3 — 'AssertionError: positive control's invocation not reached within 120.0s; last []'.
Cause 4 — 'AssertionError: SubscriptionFilter physical id is 'sim2real-cfn-sf-<id>-SubFilter-<random>', expected 'sim2real-cfn-sf-filter-<id>''.

## Steps to Reproduce
Replay the four listed cloudformation__lambda__logs checks; each deploys its template and then inspects the change-set status, the stack's terminal status, the destination function's log group, or DescribeStackResource's PhysicalResourceId.
