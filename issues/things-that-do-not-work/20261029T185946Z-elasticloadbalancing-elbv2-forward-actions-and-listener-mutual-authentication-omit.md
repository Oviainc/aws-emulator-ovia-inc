Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-lifecycle-register-targets
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-lifecycle-register-targets elbv2-https-listener-with-cert elbv2err-listener-create-rejections elasticloadbalancing-modify-listener-port-updates elbv2-describe-listeners-filtered-by-load-balancer elbv2-modify-listener-inplace-update-and-notfound elbv2-op-contract-describe-listeners elasticloadbalancing-listener-rule-path-based-routing elasticloadbalancing-rule-condition-values-mirroring
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|CreateListener|2xx->2xx|$.CreateListenerResponse.CreateListenerResult.Listeners.member.DefaultActions.member.ForwardConfig
Issue-Key: elasticloadbalancing|DescribeListeners|2xx->2xx|$.DescribeListenersResponse.DescribeListenersResult.Listeners.member.DefaultActions.member.ForwardConfig
Issue-Key: elasticloadbalancing|ModifyListener|2xx->2xx|$.ModifyListenerResponse.ModifyListenerResult.Listeners.member.DefaultActions.member.ForwardConfig
Issue-Key: elasticloadbalancing|DescribeListeners|2xx->2xx|$.DescribeListenersResponse.DescribeListenersResult.Listeners.member.MutualAuthentication
Issue-Key: elasticloadbalancing|CreateRule|2xx->2xx|$.CreateRuleResponse.CreateRuleResult.Rules.member.Actions.member.ForwardConfig
Issue-Key: elasticloadbalancing|CreateRule|2xx->2xx|$.CreateRuleResponse.CreateRuleResult.Rules.member.Transforms
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*]
Issue-Key: elasticloadbalancing|ModifyRule|2xx->2xx|$.ModifyRuleResponse.ModifyRuleResult.Rules.member.Actions.member.ForwardConfig
Issue-Key: elasticloadbalancing|ModifyRule|2xx->2xx|$.ModifyRuleResponse.ModifyRuleResult.Rules.member.Transforms
Issue-Key: elasticloadbalancing|SetRulePriorities|2xx->2xx|$.SetRulePrioritiesResponse.SetRulePrioritiesResult.Rules.member[*].Actions.member.ForwardConfig
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Actions.member.ForwardConfig
Issue-Key: elasticloadbalancing|DescribeRules|2xx->2xx|$.DescribeRulesResponse.DescribeRulesResult.Rules.member[*].Transforms
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# ELBv2 forward actions and listener mutual authentication omit their normalized config blocks

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
AWS normalizes a shorthand forward action (one that carries only TargetGroupArn) into the full ForwardConfig form and echoes BOTH representations in every reply that serializes an Action. The emulator stores and serializes only the shorthand, so ForwardConfig is absent everywhere, and an ALB HTTPS listener is additionally serialized without its MutualAuthentication block. 9 causes, one per operation/step that serializes an affected action.

AWS (applies to all 9): Smithy Action.ForwardConfig -> ForwardActionConfig{TargetGroups: TargetGroupList, TargetGroupStickinessConfig}. For a single-target-group forward action AWS returns Action.TargetGroupArn plus ForwardConfig.TargetGroups with exactly one member {TargetGroupArn: <the action's target group>, Weight: '1'} and ForwardConfig.TargetGroupStickinessConfig.Enabled 'false' — the weight 1 and the disabled stickiness are the defaults, the ARN comes from the request. Smithy Listener.MutualAuthentication -> MutualAuthenticationAttributes, returned with Mode 'off' on a listener created without mTLS. Smithy Rule.Transforms -> RuleTransformList, serialized as an empty list.
Emulator (all 9): the ForwardConfig member is absent from the serialized action (and MutualAuthentication / Transforms where noted below); everything else in the payload matches.
Why: an SDK client reading action.forward_config() gets None, so code that inspects target-group weights or stickiness (the canonical way to read a forward action, since weighted forwarding is only expressible through ForwardConfig) sees no target groups at all; terraform-style diff logic sees the config disappear and rewrites the listener/rule on every apply.
Fix: normalize a forward action into ForwardConfig when it is stored, and have the listener and rule serializers emit ForwardConfig, Listener.MutualAuthentication and Rule.Transforms.

Cause 1 of 9: CreateListener — DefaultActions.member.ForwardConfig missing. Traces: elasticloadbalancing-alb-lifecycle-register-targets#task#5 (also elbv2-https-listener-with-cert#task#8, elbv2err-listener-create-rejections#task#9/#task#10).
Cause 2 of 9: DescribeListeners — Listeners.member.DefaultActions.member.ForwardConfig missing. Traces: elasticloadbalancing-alb-lifecycle-register-targets#task#7 (+6 more steps in the cluster).
Cause 3 of 9: ModifyListener — ModifyListenerResult.Listeners.member.DefaultActions.member.ForwardConfig missing. Traces: elasticloadbalancing-modify-listener-port-updates#task#4.
Cause 4 of 9: DescribeListeners on an HTTPS listener — both DefaultActions.member.ForwardConfig AND Listeners.member.MutualAuthentication (expected {'Mode': 'off'}) missing. Traces: elbv2-https-listener-with-cert#task#9.
Cause 5 of 9: CreateRule — Rules.member.Actions.member.ForwardConfig and Rules.member.Transforms missing. Traces: elasticloadbalancing-listener-rule-path-based-routing#task#10.
Cause 6 of 9: DescribeRules — per-rule Actions.member.ForwardConfig and Transforms missing. Traces: elasticloadbalancing-listener-rule-path-based-routing#task#12.
Cause 7 of 9: ModifyRule — Rules.member.Actions.member.ForwardConfig and Transforms missing. Traces: elasticloadbalancing-listener-rule-path-based-routing#task#14.
Cause 8 of 9: SetRulePriorities — Rules.member[*].Actions.member.ForwardConfig missing. Traces: elasticloadbalancing-listener-rule-path-based-routing#task#15.
Cause 9 of 9: DescribeRules on mirrored conditions — Actions.member.ForwardConfig and Transforms missing on every rule. Traces: elasticloadbalancing-rule-condition-values-mirroring#task#6.

## Evidence
Every cause: status 200 on both sides, body diverges only by the absent member(s).
Cause 1: $.CreateListenerResponse.CreateListenerResult.Listeners.member.DefaultActions.member.ForwardConfig expected {'TargetGroups': {'member': {'TargetGroupArn': <tg arn>, 'Weight': '1'}}, 'TargetGroupStickinessConfig': {'Enabled': 'false'}}, got None.
Cause 2/3/4: same member absent under DescribeListenersResult / ModifyListenerResult; cause 4 additionally Listeners.member.MutualAuthentication expected {'Mode': 'off'}, got None.
Causes 5-9: Rules.member.Actions.member.ForwardConfig expected as above, got None; Rules.member.Transforms expected '' (empty list), got None.

## Steps to Reproduce
Replay the eight fixtures named under Traces and diff each listed step against its recorded response (replay_wire_corpus, trace set simparity-wire-aws-v1).
