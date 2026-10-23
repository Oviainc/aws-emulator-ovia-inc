Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-firewall-policy-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-firewall-policy-error-paths network-firewall-rule-group-error-paths network-firewall-rule-group-stateful-lifecycle
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->2xx|$.Message
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->2xx|$.__type
Issue-Key: network-firewall|CreateFirewallPolicy|4xx->2xx|status
Issue-Key: network-firewall|DescribeRuleGroup|4xx->2xx|$
Issue-Key: network-firewall|DescribeRuleGroup|4xx->2xx|$.Message
Issue-Key: network-firewall|DescribeRuleGroup|4xx->2xx|$.__type
Issue-Key: network-firewall|DescribeRuleGroup|4xx->2xx|status
Issue-Key: network-firewall|DescribeRuleGroup|2xx->2xx|$.RuleGroup.RulesSource.RulesString
Issue-Key: network-firewall|DescribeRuleGroup|2xx->2xx|$.RuleGroupResponse.Description
Issue-Related: 20261023T001140Z-network-firewall-responses-omit-default-encryptionconfiguration-keyid-and.md
Issue-Commit: 3e963f69e1d0897f40654d15e23b2fa6367d6b7c

# Network Firewall rule-group state mutations and policy rule-group references are not applied

> _Note: this issue was identified against the code base at commit `3e963f69e1d0897f40654d15e23b2fa6367d6b7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallPolicy

## Description
Three divergences in which the Network Firewall emulator's rule-group state does not match what the preceding call should have done to it: a reference to a rule group that does not exist is accepted, a deleted rule group is still describable, and an updated rule group still describes its pre-update contents. All three are in one service and all three leave a caller with a wrong answer about whether a resource exists or what it contains.

Cause 1 of 3: CreateFirewallPolicy accepts a StatefulRuleGroupReferences entry naming a rule group ARN that does not exist.
AWS: Smithy models CreateFirewallPolicy with required FirewallPolicyName and FirewallPolicy, and the client error InvalidRequestException (httpError 400), documented as "The operation failed because of a problem with your request. Examples include: ... Your request references an ARN that is malformed, or corresponds to a resource that isn't valid in the context of the request." A policy document whose StatefulRuleGroupReferences names a stateful-rulegroup ARN with no such rule group is therefore rejected with HTTP 400 / __type com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException, and no policy is created. The recorded rejection message "rule group ARN does not exist, parameter: [<arn>]" is an example; the requirement is the 400 status, the InvalidRequestException code, and that the rejection happens instead of a successful create.
Emulator: the request succeeds with HTTP 200 and a full FirewallPolicyResponse; __type and Message are absent because there is no error at all. The reference is never resolved against the rule-group store (the state module says so explicitly where it omits the policy's Consumed*Capacity counters).
Why: a caller gets a firewall policy that claims to reference a rule group that does not exist. Provisioning code that relies on this pre-flight — the standard way to catch a typo'd or not-yet-created rule-group ARN before attaching the policy to a firewall — sees success and proceeds, so the failure surfaces far away from the request that caused it, and negative tests that assert InvalidRequestException pass vacuously against the emulator.
Fix: CreateFirewallPolicy's request validation — resolve every StatelessRuleGroupReferences/StatefulRuleGroupReferences ARN against the rule-group store before building the record, emitting InvalidRequestException for an unresolvable reference (this is the same cross-resource resolution the Consumed*Capacity projection needs).
Traces: network-firewall-firewall-policy-error-paths#task#3.

Cause 2 of 3: a rule group survives DeleteRuleGroup — DescribeRuleGroup keeps answering 200 with an ACTIVE rule group.
AWS: Smithy models DeleteRuleGroup (output required RuleGroupResponse) and DescribeRuleGroup with the client error ResourceNotFoundException (httpError 400, doc "Unable to locate a resource using the parameters that you provided"). After a rule group is deleted, DescribeRuleGroup for that name/ARN fails with HTTP 400 / __type ...#ResourceNotFoundException; the recorded message "No object with the provided name or resource identifier was found" is an example of that response.
Emulator: the fixture's delete-then-poll teardown never converges — DescribeRuleGroup answers HTTP 200 for 11 attempts over the 60s poll cap, returning the rule group with RuleGroupStatus ACTIVE, and __type/Message are absent. The rule group is never removed from state because DeleteRuleGroup is still an unimplemented 501 scaffold, so nothing deletes it.
Why: deletion is unobservable. A client's wait-until-gone loop never terminates, teardown/idempotency flows that re-create the same rule-group name hit an "already exists" conflict instead of a clean slate, and any test that asserts ResourceNotFoundException after a delete cannot be written against the emulator.
Fix: the DeleteRuleGroup handler plus the rule-group state store it must mutate (remove the record and its ARN index entry so DescribeRuleGroup takes its not-found path).
Traces: network-firewall-rule-group-error-paths#teardown#7, network-firewall-rule-group-stateful-lifecycle#teardown#9.

Cause 3 of 3: DescribeRuleGroup returns the pre-update rules and description after UpdateRuleGroup.
AWS: Smithy models UpdateRuleGroup (required UpdateToken; optional RuleGroup/Rules/Description; output required RuleGroupResponse and a fresh UpdateToken) and its own documentation states the contract: "Updates the rule settings for the specified rule group ... first call DescribeRuleGroup to retrieve the current RuleGroup object, update the object as needed, and then provide the updated object to this call." A following DescribeRuleGroup must therefore report the NEW RuleGroup.RulesSource.RulesString and the NEW RuleGroupResponse.Description.
Emulator: after the update, DescribeRuleGroup still reports the original rules string and the original description (recorded example: expected rules "...msg:\"probe rule 2\"; sid:2..." and Description "probe stateful rule group v2", got "...msg:\"probe rule 1\"; sid:1..." and "probe stateful rule group"), with HTTP 200 on both. The update never lands because UpdateRuleGroup is still an unimplemented 501 scaffold.
Why: a rule group is write-once in practice. A caller that edits rules — the documented read-modify-write cycle, and the only way to change a deployed rule group — observes the old rules as though the edit succeeded or silently vanished, so rule changes can never be verified and optimistic-locking behaviour around UpdateToken cannot be exercised.
Fix: the UpdateRuleGroup handler and the rule-group state store (apply RuleGroup/Rules/Description/EncryptionConfiguration to the record, re-derive ConsumedCapacity and LastModifiedTime, and issue a new UpdateToken) — the stale read is in DescribeRuleGroup only in the sense that it is the first operation that can observe the missing write.
Traces: network-firewall-rule-group-stateful-lifecycle#task#7.

Relation to existing issues: the open issue "Network Firewall rule-group capacity accounting/DryRun and resource-policy statement validation are not served" covers CreateRuleGroup capacity/DryRun semantics and PutResourcePolicy statement validation; it says nothing about policy-to-rule-group reference resolution, about delete, or about update, so none of the three causes above are covered by it. The open issue "Implement network-firewall PutResourcePolicy: unimplemented scaffold stub" is unrelated.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, state_reset_failures 0).

Cause 1 — fixture network-firewall-firewall-policy-error-paths step task#3, CreateFirewallPolicy:
  expected: HTTP 400, $.__type = 'com.amazonaws.networkfirewall.customerapi.v20201112#InvalidRequestException', e.g. $.Message = 'rule group ARN does not exist, parameter: [arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/sim2real-probe-network-firewall-nfw-bogus-rg-214270bd]'
  actual:   HTTP 200, no __type, no Message (policy created)

Cause 2 — fixture network-firewall-rule-group-error-paths step teardown#7, DescribeRuleGroup (also network-firewall-rule-group-stateful-lifecycle#teardown#9):
  expected: HTTP 400, $.__type = 'com.amazonaws.networkfirewall.customerapi.v20201112#ResourceNotFoundException', e.g. $.Message = 'No object with the provided name or resource identifier was found'
  actual:   HTTP 200 with RuleGroupResponse {RuleGroupStatus: 'ACTIVE', Type: 'STATEFUL', Capacity: 30, ConsumedCapacity: 1, NumberOfAssociations: 0} and the rules still present; replay reports "poll capped at 60s" and "error code None not in ['ResourceNotFoundException'] after 11 attempt(s)".

Cause 3 — fixture network-firewall-rule-group-stateful-lifecycle step task#7, DescribeRuleGroup, expected 200 / actual 200:
  $.RuleGroup.RulesSource.RulesString: value_mismatch (expected 'alert tcp any any -> any any (msg:"probe rule 2"; sid:2; rev:1;)', got 'alert tcp any any -> any any (msg:"probe rule 1"; sid:1; rev:1;)')
  $.RuleGroupResponse.Description: value_mismatch (expected 'probe stateful rule group v2', got 'probe stateful rule group')

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): CreateFirewallPolicy (errors InvalidRequestException/InvalidOperationException/LimitExceededException/ThrottlingException, all httpError 400), DescribeRuleGroup and DeleteRuleGroup (errors include ResourceNotFoundException, httpError 400), UpdateRuleGroup (required UpdateToken, optional RuleGroup/Rules/Description, errors InvalidRequestException/InvalidTokenException/ResourceNotFoundException); shape InvalidRequestException doc ("Your request references an ARN that is malformed, or corresponds to a resource that isn't valid in the context of the request"), ResourceNotFoundException doc ("Unable to locate a resource using the parameters that you provided").
Code citations: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/create_firewall_policy.rs:208-271 (only document-size and store-count limits are checked; the record is built and inserted with no resolution of rule-group references), src/state.rs:909-925 (comment: "A policy's references are not resolved against the rule-group store yet"), src/ops/delete_rule_group.rs:3-6 and :179 (SCAFFOLDED marker; handler returns Scaffolded::not_implemented("DeleteRuleGroup")), src/ops/update_rule_group.rs:3-6 and :230 (same for UpdateRuleGroup).

## Steps to Reproduce
Cause 1:
1. aws network-firewall create-firewall-policy --firewall-policy-name p-bogus --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"],"StatefulRuleGroupReferences":[{"ResourceArn":"arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/does-not-exist"}]}'
   -> emulator returns HTTP 200 and creates the policy; AWS returns HTTP 400 InvalidRequestException and creates nothing.

Cause 2:
1. aws network-firewall create-rule-group --rule-group-name rg1 --type STATEFUL --capacity 30 --rules 'alert tcp any any -> any any (msg:"probe rule 1"; sid:1; rev:1;)'
2. aws network-firewall delete-rule-group --rule-group-name rg1 --type STATEFUL
3. aws network-firewall describe-rule-group --rule-group-name rg1 --type STATEFUL (repeat in a poll loop)
   -> emulator keeps answering HTTP 200 with RuleGroupStatus ACTIVE indefinitely; AWS answers HTTP 400 ResourceNotFoundException once the delete completes.

Cause 3:
1. Create rule group rg1 as in Cause 2 step 1 (description 'probe stateful rule group').
2. aws network-firewall describe-rule-group --rule-group-name rg1 --type STATEFUL to obtain UpdateToken.
3. aws network-firewall update-rule-group --rule-group-name rg1 --type STATEFUL --update-token <token> --description 'probe stateful rule group v2' --rules 'alert tcp any any -> any any (msg:"probe rule 2"; sid:2; rev:1;)'
4. Re-run describe-rule-group: emulator still reports 'probe rule 1' and the old description; AWS reports the updated rules and description.
