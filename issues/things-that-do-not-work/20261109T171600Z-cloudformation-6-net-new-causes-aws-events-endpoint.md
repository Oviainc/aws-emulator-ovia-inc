Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events.deploy-cfn-events-global-endpoint-lifecycle cloudformation__events.deploy-eventbus-lifecycle-then-not-found cloudformation__events.deploy-eventbus-named-default-rejected cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected cloudformation__events.deploy-cfn-events-api-destination-nonexistent-connection-rejected cloudformation__events.deploy-cfn-events-connection-and-api-destination-lifecycle
Issue-Key: cloudformation__events|CreateStack|ValidationError
Issue-Key: cloudformation__events|DescribeEventBus|ResourceNotFoundException
Issue-Key: cloudformation__events.deploy-eventbus-named-default-rejected|run#10|AssertionError|assert status == "ROLLBACK_COMPLETE", f"expected ROLLBACK_COMPLETE, got {status!r}"
Issue-Key: cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected|run#14|AssertionError|assert status == "CREATE_FAILED", f"expected stack status CREATE_FAILED, got {status!r}"
Issue-Key: cloudformation__events.deploy-cfn-events-api-destination-nonexistent-connection-rejected|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE", f"the missing-connection stack ended {status!r}"
Issue-Key: cloudformation__events.deploy-cfn-events-connection-and-api-destination-lifecycle|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# CloudFormation: 6 net-new causes — AWS::Events::Endpoint unsupported, CFN-created event bus invisible to DescribeEventBus, invalid ScheduleExpression and reserved bus name 'default' accepted, missing-connection ApiDestination accepted, stack Output emitted with no OutputValue

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Six independent CloudFormation/EventBridge-provider defects, each grounded in a failing check listed under the `cloudformation__events` heading.

Cause 1 of 6: `AWS::Events::Endpoint` is not a recognised resource type.
AWS: CreateStack accepts a template declaring `AWS::Events::Endpoint` (EventBridge global endpoint) and creates/updates/deletes it through its lifecycle; Fn::GetAtt exposes its EndpointUrl/Arn/State.
Emulator: CreateStack is refused outright with HTTP 400 `ValidationError` and the message `Template format error: Unrecognized resource types: [AWS::Events::Endpoint]`. Fixed text here is `Template format error: Unrecognized resource types: [...]`; the bracketed list is the request's unknown types.
Why: a client cannot deploy a global endpoint at all; the whole stack is rejected rather than one resource failing.
Fix: the CloudFormation resource-provider registry (add an AWS::Events::Endpoint provider) and the template type validator.
Traces: cloudformation__events.deploy-cfn-events-global-endpoint-lifecycle.

Cause 2 of 6: an event bus created by a stack is invisible to the EventBridge API.
AWS: after a stack containing `AWS::Events::EventBus` reaches CREATE_COMPLETE, `DescribeEventBus(Name=<bus>)` returns that bus; after the stack is deleted it raises `ResourceNotFoundException` with the message `Event bus <name> does not exist.` (fixed sentence, variable = the bus name).
Emulator: `DescribeEventBus` raises `ResourceNotFoundException: Event bus <name> does not exist.` while the stack is still CREATE_COMPLETE — the stack reports the bus created but no bus exists in EventBridge state.
Why: every stack-deployed EventBridge topology (rules, targets, policies on that bus) is dead on arrival, and the not-found that should prove deletion fires before deletion, so clients cannot distinguish created from deleted.
Fix: the AWS::Events::EventBus resource provider's create path (it must write the bus into EventBridge's state, not only the stack's resource table). Open issue 20261025T002357Z-events-a-cloudformation-created-aws-events-eventbus-is-invisible.md reports the same behavior under the `events` heading; this check is not carried by it.
Traces: cloudformation__events.deploy-eventbus-lifecycle-then-not-found.

Cause 3 of 6: a stack declaring an event bus named `default` is accepted.
AWS: `default` is a reserved bus name; the AWS::Events::EventBus provider fails, so the stack ends `ROLLBACK_COMPLETE` (CREATE_FAILED on that resource).
Emulator: the stack ends `CREATE_COMPLETE`.
Why: a client that accidentally names a bus `default` gets a stack that claims success while nothing was created, and a later delete can remove/shadow the real default bus.
Fix: the AWS::Events::EventBus provider's name validation (reject reserved and malformed names and surface the failure as a resource failure).
Traces: cloudformation__events.deploy-eventbus-named-default-rejected.

Cause 4 of 6: a stack whose `AWS::Events::Rule` carries an invalid ScheduleExpression is accepted.
AWS: the rule provider's PutRule is refused (`Parameter ScheduleExpression is not valid.`), so the resource ends CREATE_FAILED and the stack ends `CREATE_FAILED`/`ROLLBACK_COMPLETE`; the rule does not exist afterwards (`ResourceNotFoundException`, message `Rule <name> does not exist on EventBus default.`).
Emulator: the stack ends `CREATE_COMPLETE`.
Why: an unschedulable rule is reported as deployed, so the client believes a schedule is live that will never fire.
Fix: ScheduleExpression validation in PutRule (shared with the AWS::Events::Rule provider), and propagation of the provider error into stack status.
Traces: cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected.

Cause 5 of 6: a stack whose `AWS::Events::ApiDestination` names a non-existent connection is accepted.
AWS: CreateApiDestination fails because the ConnectionArn does not resolve, so the resource ends CREATE_FAILED and the stack ends `ROLLBACK_COMPLETE`/`CREATE_FAILED`.
Emulator: the stack ends `CREATE_COMPLETE`.
Why: the client gets a successful stack holding an api destination that can never be invoked, and the referential error is never reported.
Fix: the AWS::Events::ApiDestination provider (resolve ConnectionArn and fail the resource when it is absent).
Traces: cloudformation__events.deploy-cfn-events-api-destination-nonexistent-connection-rejected.

Cause 6 of 6: DescribeStacks returns an Output entry with no `OutputValue`.
AWS: every resolved stack output in DescribeStacks carries both `OutputKey` and `OutputValue` (an unresolvable Fn::GetAtt fails the stack instead of yielding a valueless output).
Emulator: DescribeStacks returns an `Outputs` member whose entry lacks `OutputValue` entirely — the check's `{o['OutputKey']: o['OutputValue'] ...}` raises `KeyError: 'OutputValue'` — for a stack whose outputs Fn::GetAtt the connection / api-destination attributes.
Why: any client reading stack outputs (the normal way to hand ARNs to the next step) crashes or silently sees a missing key instead of the attribute value.
Fix: the stack-output resolution/serialization path (Fn::GetAtt on AWS::Events::Connection and AWS::Events::ApiDestination attributes, and the DescribeStacks Output serializer which must never emit an Output without a value).
Traces: cloudformation__events.deploy-cfn-events-connection-and-api-destination-lifecycle.

## Evidence
Cause 1: `botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::Events::Endpoint]`.
Cause 2: `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeEventBus operation: Event bus <bus-name> does not exist.` raised while the stack is CREATE_COMPLETE.
Cause 3: `AssertionError: expected ROLLBACK_COMPLETE, got 'CREATE_COMPLETE'`.
Cause 4: `AssertionError: expected stack status CREATE_FAILED, got 'CREATE_COMPLETE'`.
Cause 5: `AssertionError: the missing-connection stack ended 'CREATE_COMPLETE'`.
Cause 6: `KeyError: 'OutputValue'` from `{o['OutputKey']: o['OutputValue'] for o in stack.get('Outputs', [])}`.

## Steps to Reproduce
1. Replay the checks named in trace_ids.
2. Cause 1: CreateStack with a template whose Resources contain a single `AWS::Events::Endpoint`.
3. Cause 2: CreateStack with one `AWS::Events::EventBus`, wait for CREATE_COMPLETE, then call `events.describe_event_bus(Name=<bus>)`.
4. Cause 3: CreateStack with `AWS::Events::EventBus` whose Name is `default`; read the final stack status.
5. Cause 4: CreateStack with an `AWS::Events::Rule` whose ScheduleExpression is syntactically invalid; read the final stack status.
6. Cause 5: CreateStack with an `AWS::Events::ApiDestination` whose ConnectionArn names a connection that was never created.
7. Cause 6: CreateStack with `AWS::Events::Connection` + `AWS::Events::ApiDestination` and Outputs that Fn::GetAtt their attributes, then call DescribeStacks and inspect each Outputs entry for `OutputValue`.
