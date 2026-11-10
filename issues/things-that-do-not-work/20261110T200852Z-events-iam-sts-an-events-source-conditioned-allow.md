Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__iam__sts.put-rule-source-conditions-gate-the-event-pattern
Issue-Key: events__iam__sts.put-rule-source-conditions-gate-the-event-pattern|_until_admitted#11|Timeout|time.sleep(3)
Issue-Commit: 72bb6bfd937a489e314e0b070a9ac1619964b2b9

# events__iam__sts: an events:source-conditioned Allow never admits PutRule — the permitted event pattern is refused AccessDeniedException forever and the check exhausts its 60s budget

> _Note: this issue was identified against the code base at commit `72bb6bfd937a489e314e0b070a9ac1619964b2b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Cause 1 of 1: PutRule under an assumed role whose only Allow is conditioned on the event-pattern request key `events:source` is refused for a pattern that satisfies the condition, so the conditioned Allow never grants anything.

AWS: Smithy: eventbridge#PutRule takes Name (required, @length 1..64, pattern ^[\.\-_A-Za-z0-9]+$), EventBusName, EventPattern (@length 0..4096) and answers PutRuleResponse{RuleArn}; its modeled errors are ConcurrentModificationException, InternalException, InvalidEventPatternException, LimitExceededException, ManagedRuleException, ResourceNotFoundException — AccessDeniedException is the authorization refusal, a service fact the model does not carry, so for the authorization contract this is recorded behavior only. AWS authorizes PutRule with the pattern-derived keys (`events:source`, `events:detail-type`) published into the request context. Hence: a statement `Allow events:PutRule on *` with `Condition {"Null": {"events:source": "false"}}` MATCHES a PutRule whose EventPattern has a source — the call is served 200 with a RuleArn and DescribeRule reads the pattern back verbatim — and does NOT match one whose EventPattern has only detail-type, which is refused `AccessDeniedException` and creates no rule (DescribeRule then answers `ResourceNotFoundException` with the fixed text `Rule ` + name + ` does not exist on EventBus ` + bus + `.`). Likewise a statement conditioned `ForAllValues:StringEquals events:source = <allowed>` admits a pattern whose source is <allowed> and refuses one naming another source. The refusal is AWS's standard implicit-deny sentence: fixed text `User: ` + principal + ` is not authorized to perform: ` + action + ` on resource: ` + resource + ` because no identity-based policy allows the ` + action + ` action`, whose variable parts are the assumed-role session ARN (arn:aws:sts::<account>:assumed-role/<role>/<session>), the action `events:PutRule` and the rule ARN arn:aws:events:<region>:<account>:rule/<bus>/<rule> built from the request.

Emulator: the ADMITTED leg is refused. The harness's bounded retry around the first PutRule (the sourced pattern, which the Null condition allows) re-tries only that exact AccessDeniedException sentence or a not-yet-valid session token, and it never stopped re-trying: the check consumed its whole budget — `Error: Check timed out after 60.0s` — without reaching an assertion. Hypothesis: the authorization gate does not publish the pattern-derived condition keys (`events:source`) into the request context for PutRule, or does not implement the Null / ForAllValues:StringEquals operators over them, so a conditioned Allow matches no request and every PutRule by that role is refused. This is the INVERSE symptom of the already-open 20261109T163411Z-events-iam-sts-eventbridge-putrule-is-not-gated-by-the-event.md, which recorded the condition being ignored so that a DISALLOWED source was accepted 200; its replay key is that 'expected the call to be refused, but it succeeded' assertion, and the suite does not attribute this failure to it. The permitted call is now refused instead — a second, opposite defect in the same gate that must be closed with it.

Why: a client that scopes rule authoring per tenant ('this role may only create rules for its own source') loses the capability entirely: every PutRule it makes, including those its policy explicitly allows, comes back AccessDeniedException. Because the refusal is worded exactly like AWS's propagation-window denial, a client's retry-until-the-policy-lands loop never terminates and the deployment hangs rather than failing.

Fix: the PutRule authorization step's condition evaluation — it must derive and publish `events:source` (and `events:detail-type`) from the submitted EventPattern into the authorization context and evaluate Null / ForAllValues:StringEquals against them, so a matching statement grants and only a non-matching one refuses.

Traces: events__iam__sts.put-rule-source-conditions-gate-the-event-pattern.

## Evidence
Check events__iam__sts.put-rule-source-conditions-gate-the-event-pattern, [mismatch].

Expected (AWS): with the role's only statement being Allow events:PutRule on * Condition Null events:source = false, PutRule(EventPattern={"source":[<src>]}) is served 200 and DescribeRule reads back that pattern; PutRule(EventPattern={"detail-type":[...]}) is refused AccessDeniedException 'User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: events:PutRule on resource: arn:aws:events:<region>:<account>:rule/<bus>/<rule> because no identity-based policy allows the events:PutRule action' and DescribeRule then answers ResourceNotFoundException 'Rule <rule> does not exist on EventBus <bus>.' The ForAllValues:StringEquals events:source leg behaves the same way for the allowed vs. other source.

Actual (emulator): Error: Check timed out after 60.0s. The only loop reached at that point retries exactly that AccessDeniedException sentence (or the not-yet-valid-token refusal) while waiting for the conditioned Allow to admit the sourced PutRule; it never became admitted, so the permitted call stays refused and no assertion is ever reached.

## Steps to Reproduce
1. CreateEventBus.
2. CreateRole trusting arn:aws:iam::<account>:root with one inline policy: {Effect: Allow, Action: events:PutRule, Resource: "*", Condition: {"Null": {"events:source": "false"}}}. AssumeRole it.
3. With the assumed-role credentials call PutRule(Name=<rule>, EventBusName=<bus>, EventPattern={"source":["sim2real.<x>"]}). AWS serves 200 and DescribeRule returns that pattern; the emulator refuses it AccessDeniedException indefinitely (retrying never admits it).
4. With the same credentials call PutRule with EventPattern={"detail-type":["sim2real.detail"]} — this one must be the AccessDeniedException, and DescribeRule on that name must answer ResourceNotFoundException 'Rule <rule> does not exist on EventBus <bus>.'
5. Repeat 2-4 with Condition {"ForAllValues:StringEquals": {"events:source": "<allowed>"}}: the <allowed>-source pattern must be admitted, another source refused.
