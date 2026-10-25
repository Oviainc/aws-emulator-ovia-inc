Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.alb-listener-target-group-lifecycle elbv2.listener-default-action-empty-vs-absent-fields-preserved
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2|DeleteRule|NotImplemented
Issue-Key: elbv2.listener-default-action-empty-vs-absent-fields-preserved|run#13|AssertionError|assert path_cond.get("Values") == ["/only-config"], (
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# ELBv2 rule surface: DeleteRule answers 501 NotImplemented, and DescribeRules does not mirror PathPatternConfig.Values into the legacy top-level Values

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRule

## Description
TWO distinct net-new root causes are bundled here, both in the elasticloadbalancingv2 listener-rule surface; both must be closed before this issue is done. They are grounded in two different failing behavioral checks and are independent of each other.

Neither is covered by an open issue. The open issue 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md bundles CreateRule, DeleteTargetGroup, tagging, trust stores etc. — it names CreateRule (now implemented) but NOT DeleteRule, and its Cause 4 claims the listener-default-action representation check is merely gated behind the CreateRule stub; CreateRule now succeeds and the check fails for a different reason (Cause 2 below), so that issue's description does not describe this defect and the replay suite did not attribute either check to it.

Cause 1 of 2 — DeleteRule is an unimplemented scaffold stub, so a listener rule can never be deleted.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#DeleteRule(RuleArn: RuleArn, required) -> DeleteRuleOutput, an empty 200 body. Its modeled errors are exactly two, both HTTP 400: RuleNotFoundException (awsQueryError code 'RuleNotFound') and OperationNotPermittedException (awsQueryError code 'OperationNotPermitted'); the model's docs state "Deletes the specified rule. You can't delete the default rule", which is the OperationNotPermitted case. A successful delete removes the rule, so the target group the rule forwarded to is no longer referenced and DeleteTargetGroup stops being refused.
Emulator: every DeleteRule call is answered HTTP 501 with Error.Code=NotImplemented (message, as an example: "operation DeleteRule is scaffolded but not implemented in this emulator"). The handler body still returns the scaffold's not_implemented value.
Why: a client cannot tear down listener routing. Rules accumulate forever, the rule-reference guard that pins a target group can never be released, so the whole create -> read -> delete lifecycle (rule, listener, target group, load balancer) dead-ends, and a client cannot distinguish "rule not found" / "cannot delete the default rule" from an unavailable service because 501/NotImplemented is not a modeled code it can branch on.
Fix: the DeleteRule handler must remove the rule from the per-listener rule store CreateRule writes and DescribeRules walks, and raise the two modeled 400 errors. Traces below cite the file and line.
Traces: elbv2.alb-listener-target-group-lifecycle (the check creates an internal ALB, an ip target group, an HTTP:80 fixed-response listener and a forward rule, then deletes each; it dies at the DeleteRule step). Stub marker and handler: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/delete_rule.rs line 4 (SCAFFOLDED marker) and line 147 (the returned not_implemented error).

Cause 2 of 2 — DescribeRules drops the legacy top-level Values for a path-pattern condition created through PathPatternConfig.
AWS: Smithy RuleCondition has both a legacy top-level Values (ListOfString) and the newer per-field config structures; the model documents Values as "The condition value. Specify only when Field is host-header or path-pattern. Alternatively, to specify multiple host names or multiple path patterns, use HostHeaderConfig or PathPatternConfig" — i.e. PathPatternConditionConfig.Values and RuleCondition.Values are two spellings of the same array. Recorded behavior: a rule created with only Conditions=[{Field: 'path-pattern', PathPatternConfig: {Values: [<pattern>]}}] reads back from DescribeRules with the config value MIRRORED into the legacy top-level Values for backward compatibility, while a condition whose field never had a legacy form (Field: 'http-header', configured via HttpHeaderConfig) reads back with NO top-level Values key at all — present-but-empty and absent are distinct.
Emulator: for the path-pattern condition created via PathPatternConfig only, DescribeRules returns the condition with no top-level Values (the check reads None where the mirrored array is required). The http-header half of the distinction is never reached because the path-pattern assertion fails first.
Why: every client and tool that reads rule conditions through the legacy Values field — the pre-config SDK shape, console-style diffing, CloudFormation/Terraform drift detection, any code that renders a rule's patterns — sees a path-pattern rule with no patterns at all, so a correctly configured rule looks empty and a drift check reports a false change.
Fix: the rule-condition representation in the elasticloadbalancingv2 crate — either normalize PathPatternConfig/HostHeaderConfig into the legacy Values when the rule is stored (src/ops/create_rule.rs condition handling) or mirror it when the condition is serialized (the DescribeRules handler), and in both cases keep Values genuinely ABSENT for fields that have no legacy form (http-header, query-string, http-request-method, source-ip) rather than emitting an empty array.
Traces: elbv2.listener-default-action-empty-vs-absent-fields-preserved.

## Evidence
Cause 1 — elbv2.alb-listener-target-group-lifecycle [error]:
Expected: DeleteRule(RuleArn=<rule>) -> HTTP 200 empty DeleteRuleOutput (or 400 RuleNotFound / 400 OperationNotPermitted).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteRule operation: operation DeleteRule is scaffolded but not implemented in this emulator. HTTP 501.
Smithy citation: DeleteRule input {RuleArn required}, output DeleteRuleOutput (empty), errors [RuleNotFoundException -> awsQueryError {code: RuleNotFound, httpResponseCode: 400}, OperationNotPermittedException -> awsQueryError {code: OperationNotPermitted, httpResponseCode: 400}].
Source citation: services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/delete_rule.rs:4 '//! SCAFFOLDED, NOT IMPLEMENTED' and :147 'Err(crate::scaffolded::Scaffolded::not_implemented("DeleteRule"))'.

Cause 2 — elbv2.listener-default-action-empty-vs-absent-fields-preserved [mismatch]:
Expected: DescribeRules(ListenerArn=<listener>) returns the path-pattern condition with Values == the array supplied in PathPatternConfig.Values.
Actual: AssertionError: expected path-pattern Values mirrored from PathPatternConfig, got None (check line 63).
Smithy citation: RuleCondition.Values (ListOfString) doc — "Specify only when Field is host-header or path-pattern. Alternatively ... use HostHeaderConfig or PathPatternConfig"; PathPatternConditionConfig.Values (ListOfString).

## Steps to Reproduce
Shared setup: boot the emulator; create a VPC with two subnets in different AZs, an internal application load balancer over them, an ip target group in that VPC, and an HTTP:80 listener.

Cause 1 (DeleteRule 501):
1. CreateRule(ListenerArn=<listener>, Priority=10, Conditions=[{Field:'path-pattern', PathPatternConfig:{Values:['/x']}}], Actions=[{Type:'forward', TargetGroupArn:<tg>}]) -> 200, note the RuleArn.
2. DeleteRule(RuleArn=<that arn>) -> observe HTTP 501 Error.Code=NotImplemented; AWS answers 200 with an empty body.
3. DeleteRule(RuleArn=<a well-formed but nonexistent rule arn>) -> observe 501; AWS answers 400 RuleNotFound.
4. Re-replay elbv2.alb-listener-target-group-lifecycle.

Cause 2 (PathPatternConfig not mirrored):
1. CreateRule(... Priority=10, Conditions=[{Field:'path-pattern', PathPatternConfig:{Values:['/only-config']}}] ...) with NO top-level Values supplied.
2. CreateRule(... Priority=20, Conditions=[{Field:'http-header', HttpHeaderConfig:{HttpHeaderName:'X-Probe', Values:['probe']}}] ...).
3. DescribeRules(ListenerArn=<listener>): the path-pattern rule's condition has no top-level Values (AWS mirrors ['/only-config'] there), and the http-header rule's condition must carry no Values key at all.
4. Re-replay elbv2.listener-default-action-empty-vs-absent-fields-preserved.
