Issue-Source: trace-sweep-divergence
Issue-Trace: elbv2rule-default-rule-immutable
Issue-Step: task#12
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbv2rule-default-rule-immutable
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|SetRulePriorities|2xx->2xx|$.SetRulePrioritiesResponse.SetRulePrioritiesResult.Rules.member.Transforms
Issue-Commit: 610eb4b5abbe12768656cecbe381c20cd7e570c7

# ELBv2 SetRulePriorities response emits an empty Transforms element AWS omits

> _Note: this issue was identified against the code base at commit `610eb4b5abbe12768656cecbe381c20cd7e570c7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetRulePriorities

## Description
The Rule serializer used on the SetRulePriorities response path emits a Transforms element for rules that have no transforms, where AWS omits the member entirely. 1 cause.

Cause 1 of 1: SetRulePriorities — SetRulePrioritiesResult.Rules.member.Transforms is present (empty) in the emulator reply and absent in AWS's.
AWS: Smithy elasticloadbalancingv2#SetRulePriorities -> output SetRulePrioritiesOutput.Rules -> Rules (list of Rule); Rule.Transforms -> RuleTransformList, documented 'The transforms for the rule.' The member is modeled but is NOT serialized by AWS on this response path when the rule carries no transforms: the recorded SetRulePriorities reply (HTTP 200) contains Rules.member with RuleArn, Priority, Conditions, Actions and IsDefault and no Transforms element at all. Recorded behavior only for the operation-specific split: the same corpus shows DescribeRules replies that DO carry an empty <Transforms/> on each rule, so the omission is specific to the SetRulePriorities (rule-priority mutation) response, not to the Rule shape in general. No error code or message is involved — both sides answer 200 and only this member diverges.
Emulator: serializes an empty <Transforms/> inside every SetRulePrioritiesResult.Rules.member. Replay verdict: $.SetRulePrioritiesResponse.SetRulePrioritiesResult.Rules.member.Transforms: unexpected_present (expected None, got '').
Why: an awsQuery/XML client that distinguishes an absent member from a present-but-empty list reads Some([]) where AWS gives None. Code that treats a present Transforms as 'this rule has a transform block' — or that round-trips the returned Rule straight back into ModifyRule — then carries a block AWS never returned, and any assertion that SetRulePriorities and DescribeRules agree member-for-member with AWS fails on this element.
Fix: the SetRulePriorities response serializer (the Rule serialization used on that path) — suppress the Transforms element when the rule has no transforms, matching the recorded AWS reply; leave the DescribeRules path, which does emit it, alone.
Traces: elbv2rule-default-rule-immutable#task#12.

Related open issues (neither covers this member; both are about the opposite direction or a different path): 'ELBv2 load balancer and target health payloads drop defaulted members and mis-form DNS names' (omitted members on LoadBalancer/TargetHealthDescription) and 'ELBv2 DescribeRules paginates by default and reorders rules, conditions and redirect config' (DescribeRules ordering/pagination).

## Evidence
Fixture elbv2rule-default-rule-immutable, step task#12, operation SetRulePriorities, service elasticloadbalancing.
Expected status 200; actual status 200 — status and error-free shape agree.
Diverging member: $.SetRulePrioritiesResponse.SetRulePrioritiesResult.Rules.member.Transforms — expected None (element absent in the recorded AWS reply), got '' (an empty element emitted by the emulator).
The diverging thing is the presence of the element, not any value inside it: no id, ARN, timestamp or request id from the recording is part of the requirement.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator (replay_wire_corpus).
2. Look at fixture elbv2rule-default-rule-immutable, step task#12: it creates a listener with its default rule plus an extra non-default rule and then calls SetRulePriorities on them.
3. Diff the SetRulePriorities response against the recorded one: the emulator's SetRulePrioritiesResult.Rules.member contains an empty <Transforms/> element that the recorded AWS reply does not contain.
