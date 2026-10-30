Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2-describe-rules-returns-priority-ordered
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2-describe-rules-returns-priority-ordered elbv2rule-action-enum-and-pattern-validation elbv2rule-condition-combination-limits elbv2rule-condition-legacy-values-mirroring elbv2rule-default-rule-immutable elbv2rule-priority-range-boundaries elbv2rule-redirect-component-pattern-validation elbv2rule-redirect-omitted-components-default-to-placeholders elbv2rule-redirect-self-loop-rejected elbv2env-listener-attributes-targets-and-rule-pagination
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member.Transforms
Issue-Key: elasticloadbalancing|CreateRule|2xx->2xx|$.CreateRuleResponse.CreateRuleResult.Rules.member.Actions.member.FixedResponseConfig.ContentType
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member.Actions.member.FixedResponseConfig.ContentType
Issue-Related: 20261029T185946Z-elasticloadbalancing-elbv2-forward-actions-and-listener-mutual-authentication-omit.md
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 rule responses omit Transforms and the FixedResponseConfig ContentType default

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
Every ELBv2 reply that serializes a Rule must carry the Transforms member, and a fixed-response action must carry the ContentType the service defaulted. The emulator's rule serializer emits neither. 8 causes, one per operation/step that serializes a rule.

AWS (all causes): Smithy Rule.Transforms -> RuleTransformList ('The transforms for the rule'); a rule created without transforms is still serialized with Transforms present and empty (the awsQuery wire form of an empty list). Smithy Action.FixedResponseConfig -> FixedResponseActionConfig; when a fixed-response action is created with only StatusCode (and optional MessageBody), AWS stores and echoes ContentType 'text/plain' — the default is fixed text, the StatusCode/MessageBody are the request's values.
Emulator: Rules.member.Transforms is absent from every rule payload, and FixedResponseConfig is echoed without ContentType.
Why: a client that round-trips a described rule back into ModifyRule loses the transforms member entirely, and config-management clients see ContentType flip between 'text/plain' (AWS) and unset (emulator), producing a permanent spurious diff on every fixed-response rule.
Fix: the rule serializer (Transforms always emitted) and the CreateRule/ModifyRule action normalization that defaults FixedResponseConfig.ContentType.

Cause 1 of 8: CreateRule — Rules.member.Transforms missing. This cluster alone covers 20 recorded steps. Traces: elbv2-describe-rules-returns-priority-ordered#task#3.
Cause 2 of 8: DescribeRules — Transforms missing on all four returned rules. Traces: elbv2-describe-rules-returns-priority-ordered#task#6.
Cause 3 of 8: DescribeRules after action enum/pattern validation — Transforms missing on all three rules. Traces: elbv2rule-action-enum-and-pattern-validation#task#12.
Cause 4 of 8: DescribeRules on a listener with its default rule — Transforms missing on both rules. Traces: elbv2rule-default-rule-immutable#task#8.
Cause 5 of 8: ModifyRule — ModifyRuleResult.Rules.member.Transforms missing. Traces: elbv2rule-default-rule-immutable#task#11.
Cause 6 of 8: DescribeRules after a modify — Transforms missing. Traces: elbv2rule-default-rule-immutable#task#13.
Cause 7 of 8: CreateRule with a fixed-response action — Transforms missing AND Actions.member.FixedResponseConfig.ContentType missing (expected 'text/plain'). Traces: elbv2rule-redirect-omitted-components-default-to-placeholders#task#10.
Cause 8 of 8: DescribeRules on fixed-response rules — Transforms missing AND Actions.member.FixedResponseConfig.ContentType missing. Traces: elbv2env-listener-attributes-targets-and-rule-pagination#task#21.

## Evidence
All causes: status 200 expected and returned.
Transforms: $...Rules.member[*].Transforms expected '' (empty list), got None — on CreateRule, ModifyRule and DescribeRules alike.
ContentType: $...Rules.member.Actions.member.FixedResponseConfig.ContentType expected 'text/plain', got None (causes 7 and 8).

## Steps to Reproduce
Replay the seven fixtures named under Traces and diff each listed step against its recorded response (replay_wire_corpus, trace set simparity-wire-aws-v1).
