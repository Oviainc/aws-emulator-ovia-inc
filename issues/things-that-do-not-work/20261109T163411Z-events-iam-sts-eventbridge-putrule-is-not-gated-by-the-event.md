Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__iam__sts.put-rule-source-conditions-gate-the-event-pattern
Issue-Key: events__iam__sts.put-rule-source-conditions-gate-the-event-pattern|_error#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# EventBridge PutRule is not gated by the event-pattern request conditions: a rule whose pattern falls outside the allowed sources is accepted

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: PutRule's authorization does not publish the request's event-pattern-derived condition keys (events:source / events:detail-type), so a policy that restricts which patterns a principal may create does not constrain it.

AWS: PutRule is authorized with the event-pattern condition keys in scope. A principal whose policy allows events:PutRule only for a named source is refused when it submits an EventPattern whose source is not in that set: the call fails with `AccessDeniedException` carrying AWS's standard no-matching-policy wording — `User: <principal arn> is not authorized to perform: events:PutRule on resource: <rule arn> because no identity-based policy allows the events:PutRule action` — with principal, action and resource taken from the request. Smithy: eventbridge#PutRule takes Name, EventBusName and EventPattern and returns PutRuleResponse{RuleArn}; the condition-key set is a service fact — recorded behavior only, and the recording shows the out-of-scope pattern refused.

Emulator: the call succeeds, returning `{'RuleArn': 'arn:aws:events:<region>:<account>:rule/<bus>/<rule>'}` with HTTP 200.

Why: a client that gates rule authoring by event source (so a tenant may only subscribe to its own events) has no control: any principal that may create rules at all can subscribe to every source on the bus, which is an information-disclosure path. The allowed leg also succeeds, so the client cannot distinguish an enforced policy from an inert one.

Fix: the PutRule authorization step must publish the pattern-derived condition keys into the authorization context and refuse when no statement matches.

Traces: events__iam__sts.put-rule-source-conditions-gate-the-event-pattern.

## Evidence
Expected: PutRule with an EventPattern outside the permitted source set is refused.
Actual: AssertionError: expected the call to be refused, but it succeeded: {'RuleArn': 'arn:aws:events:us-west-2:000000000000:rule/sim2real-behav-bus-…/sim2real-behav-rule-…', …HTTPStatusCode: 200}

## Steps to Reproduce
1. Create an event bus and a role whose policy allows events:PutRule only when the pattern's source is a named value; AssumeRole.
2. Call PutRule with an EventPattern naming a different source.
3. Observe a 200 with a RuleArn instead of AccessDeniedException.
