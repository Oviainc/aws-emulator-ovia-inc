Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elastic-load-balancing-v2|DescribeRules|NotImplemented
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Implement elastic-load-balancing-v2 DescribeRules: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution (what landed here, and what this record does NOT own)

**The operation is implemented, and so is the store it reads.** `DescribeRules` carries no
scaffold header and answers 200 from the service's own state: a listener's rules are its
default rule plus the rules `CreateRule` persisted on it, reported in evaluation order
(ascending priority, default rule last), selectable by `RuleArns` and paged by
`Marker`/`PageSize`. `CreateRule` — the only writer of a listener's non-default rules, and
until now a scaffold stub whose 501 meant this read could never report more than one rule —
is implemented with it: priority unique per listener (`PriorityInUse`), a forward resolved
against the target group store (`TargetGroupNotFound`), the condition-field and
condition-value rules the model states in prose, and the published
`rules-per-application-load-balancer` bound (`TooManyRules`). A target group a RULE forwards
to is now in use for `DeleteTargetGroup` and reports its load balancer, through one helper on
the listener record. Pinned end to end over the wire by
`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/tests/all/listener_rules.rs`.

**The one recorded unit is NOT claimed as passing, and its remaining blocker is another
service's.** `elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection` deploys a
CloudFormation stack and then calls `DescribeRules` with the listener ARN read out of that
stack's `Outputs`. CloudFormation in this emulator does not provision ELBv2 resources through
the ELBv2 service: it synthesizes each declared resource's identifier from the template
(`services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs`, and
`src/state/provisioner.rs` has no ELBv2 arm), so the ARN that reaches this read names a
listener no `CreateListener` ever minted and `ListenerNotFound`/400 is the correct answer for
it. Serving it from the ELBv2 side would mean inventing a listener the store never created.
That work is owned by the open records
`20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md` and
`20261023T025808Z-cloudformation-elbv2-a-cloudformation-stack-whose-elbv2-listener-rules-collide.md`
(whose own stated fix is the ELBv2 `CreateRule` priority-uniqueness refusal — which landed
here — plus CloudFormation's resource-failure/rollback state machine), exactly as the resolved
record `20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md` already
reassigned this trace id.

## Operation
DescribeRules

## Description
`DescribeRules` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_rules.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeRules` call, so each one unblocks only when `DescribeRules` works: `elbv2.deploy-elbv2-1-rule-update-tg-replace-deletion-protection`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_rules.rs`. Replaying the corpus, the emulator refused the `DescribeRules` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeRules operation: operation DescribeRules is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the elastic-load-balancing-v2 emulator
Call DescribeRules and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
