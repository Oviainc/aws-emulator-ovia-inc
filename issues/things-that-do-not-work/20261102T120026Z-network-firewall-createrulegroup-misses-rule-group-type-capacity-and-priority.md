Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-nfwr-traj-stateless-rule-priority-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-nfwr-traj-stateless-rule-priority-mistakes network-firewall-nfwr-traj-suricata-rule-syntax-refusals network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateRuleGroup|4xx->2xx|$.Message
Issue-Key: network-firewall|CreateRuleGroup|4xx->2xx|$.__type
Issue-Key: network-firewall|CreateRuleGroup|4xx->2xx|status
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.Message
Issue-Key: network-firewall|CreateRuleGroup|4xx->4xx|$.__type
Issue-Key: network-firewall|CreateRuleGroup|2xx->4xx|$.RuleGroupResponse
Issue-Key: network-firewall|CreateRuleGroup|2xx->4xx|$.UpdateToken
Issue-Key: network-firewall|CreateRuleGroup|2xx->4xx|status
Issue-Commit: c1ca043e5c77dd90c2a96695b3dfa06a20838f7d

# CreateRuleGroup misses rule-group type, capacity and priority validation and then blocks the valid create

> _Note: this issue was identified against the code base at commit `c1ca043e5c77dd90c2a96695b3dfa06a20838f7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRuleGroup

## Description
CreateRuleGroup does not apply Network Firewall's semantic rule-group checks (rule type consistency, capacity sufficiency, stateless rule priority uniqueness). Requests AWS refuses with InvalidRequestException are accepted or refused with the wrong message, and the knock-on state then rejects the request AWS accepts. This bundle has 4 causes.

Cause 1 of 4 - duplicate stateless rule Priority is accepted (fixture network-firewall-nfwr-traj-stateless-rule-priority-mistakes, step task#1).
AWS: within RuleGroup.RulesSource.StatelessRulesAndCustomActions.StatelessRules, Priority must be unique (Smithy: Priority is an integer with range 1..65535; uniqueness is enforced semantically, recorded behavior). A duplicate is refused with HTTP 400, __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException', Message 'Priority has duplicate values, parameter: [10], context: StatelessRulesAndCustomActions.StatelessRules[Priority=10]'. Fixed text: 'Priority has duplicate values, parameter: [', '], context: StatelessRulesAndCustomActions.StatelessRules[Priority=', ']'. Variable parts: the duplicated priority value from the request (10 in this recording) in both the parameter list and the context path.
Emulator: HTTP 200 with a created rule group; no __type, no Message.

Cause 2 of 4 - an invalid rule-group Type is refused with the wrong error class and no message (fixture network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes, step task#1; same cluster: task#5).
AWS: CreateRuleGroup.Type is required and targets RuleGroupType (Smithy enum values STATELESS, STATEFUL, STATEFUL_DOMAIN). When the supplied Type is a modeled enum value but inconsistent with the RuleGroup contents, AWS answers HTTP 400 with __type 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException' and Message "The rule group type isn't valid. Enter a valid type, and try again." - fixed text in full, no interpolated values.
Emulator: HTTP 400 with __type 'ValidationException' (a shape this service does not model; Smithy common_errors are InternalServerError, InvalidRequestException, ThrottlingException) and no Message member at all.

Cause 3 of 4 - insufficient Capacity produces the wrong message (fixture network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes, step task#3; same cluster: task#4 and steps in network-firewall-nfwr-traj-stateless-rule-priority-mistakes and network-firewall-nfwr-traj-suricata-rule-syntax-refusals).
AWS: Capacity is required (Smithy: CreateRuleGroup.Capacity -> RuleCapacity, required) and must cover the capacity the submitted rules consume. When it does not, AWS answers HTTP 400 InvalidRequestException with Message 'StatefulRules capacity exceeded, parameter: [1], context: RulesSource.StatefulRules'. Fixed text: '<RuleKind> capacity exceeded, parameter: [', '], context: '. Variable parts: the rule kind and the context path for the rules source actually over capacity, and the Capacity value from the request (1 in this recording).
Emulator: HTTP 400 but Message 'Capacity must be at least 1 capacity unit; this request asked for 0.' - a different rule (a floor on the requested capacity) with its own wording, and it reports a computed requirement of 0, i.e. it is not computing consumed capacity from the rules at all.

Cause 4 of 4 - the valid CreateRuleGroup that follows is refused (fixture network-firewall-nfwr-traj-stateless-rule-priority-mistakes, step task#6; same cluster: network-firewall-nfwr-traj-suricata-rule-syntax-refusals#task#5).
AWS: after the refusals above, the rule-group name is still free, so the corrected request succeeds with HTTP 200 returning UpdateToken and RuleGroupResponse (Smithy: CreateRuleGroup output members UpdateToken and RuleGroupResponse are both required; the recording shows Type STATELESS, RuleGroupStatus ACTIVE, Capacity as requested and ConsumedCapacity computed from the rules).
Emulator: HTTP 400 with neither member - the rule group it wrongly created in Cause 1 now occupies the name.

Why: clients use CreateRuleGroup as the gate on rule authoring. Accepting duplicate priorities or an under-provisioned Capacity means invalid rule groups enter the emulator's state and the client's validation-error path is never exercised; an unmodeled 'ValidationException' with no Message breaks SDK error matching entirely (the SDK cannot map it to InvalidRequestException, and err.message() is empty). Cause 4 is worse than a missing refusal: a correct request fails, so a client's retry-after-fix flow cannot complete.

Fix: CreateRuleGroup's validation in the network-firewall service crate - stateless priority uniqueness, type/contents consistency, and consumed-vs-requested capacity, each raising InvalidRequestException with the fully qualified __type and the message templates above; and stop admitting rule groups that failed validation into state (which is what breaks Cause 4). Re-replay all four listed bindings after the fix.

## Evidence
stateless-rule-priority-mistakes#task#1 (and suricata-rule-syntax-refusals#task#1): expected 400 __type ...#InvalidRequestException + Message 'Priority has duplicate values, parameter: [10], context: StatelessRulesAndCustomActions.StatelessRules[Priority=10]'; actual 200 with no error members.
rule-group-capacity-and-type-mistakes#task#1 (and task#5): expected __type ...#InvalidRequestException + Message "The rule group type isn't valid. Enter a valid type, and try again."; actual __type 'ValidationException', Message missing.
rule-group-capacity-and-type-mistakes#task#3 (7-member cluster): expected Message 'StatefulRules capacity exceeded, parameter: [1], context: RulesSource.StatefulRules'; actual 'Capacity must be at least 1 capacity unit; this request asked for 0.'
stateless-rule-priority-mistakes#task#6 (and suricata-rule-syntax-refusals#task#5): expected 200 with UpdateToken + RuleGroupResponse (RuleGroupStatus ACTIVE, Type STATELESS, Capacity 10, ConsumedCapacity 2); actual 400, both members missing.

## Steps to Reproduce
Replay fixtures network-firewall-nfwr-traj-stateless-rule-priority-mistakes (task#1, task#6), network-firewall-nfwr-traj-rule-group-capacity-and-type-mistakes (task#1, task#3) and network-firewall-nfwr-traj-suricata-rule-syntax-refusals. Equivalently: CreateRuleGroup with two StatelessRules sharing Priority; CreateRuleGroup with a Type inconsistent with RuleGroup contents; CreateRuleGroup with Capacity below what the rules consume; then CreateRuleGroup with the same name corrected.
