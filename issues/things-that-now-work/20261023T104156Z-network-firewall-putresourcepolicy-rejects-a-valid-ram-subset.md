Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-resource-policy-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|PutResourcePolicy|2xx->4xx|$
Issue-Key: network-firewall|PutResourcePolicy|2xx->4xx|status
Issue-Key: network-firewall|DescribeResourcePolicy|2xx->4xx|$.Policy
Issue-Key: network-firewall|DescribeResourcePolicy|2xx->4xx|status
Issue-Key: network-firewall|DeleteResourcePolicy|2xx->4xx|$
Issue-Key: network-firewall|DeleteResourcePolicy|2xx->4xx|status
Issue-Commit: b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf

# Network Firewall PutResourcePolicy rejects a valid RAM subset policy, blocking the resource-policy lifecycle

> _Note: this issue was identified against the code base at commit `b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
The Network Firewall resource-policy lifecycle (attach -> read -> detach) cannot be driven against the emulator at all: the attach call is refused 400, and the two reads that follow it therefore also answer 400 where AWS answers 200. THREE causes, one service; causes 2 and 3 are the downstream steps of the same recorded lifecycle and must be re-replayed after cause 1 is fixed, because each is its own replay cluster.

Cause 1 of 3: PutResourcePolicy demands SET EQUALITY of the shared resource type's action list and so refuses a policy that grants a SUBSET of it.
AWS: Smithy (com.amazonaws.networkfirewall#PutResourcePolicy, awsJson1_0) models PutResourcePolicyRequest$Policy (PolicyString, required, length 1..395000, pattern \S) and documents the actions that MAY appear in the statement: "For a rule group resource, you can specify the following operations in the Actions section of the statement: network-firewall:CreateFirewallPolicy, network-firewall:UpdateFirewallPolicy, network-firewall:ListRuleGroups". That is a permitted set, not a required set — the documented error InvalidResourcePolicyException is "The policy statement failed validation." The recording confirms the rule: a rule-group policy whose single Allow statement grants only [network-firewall:CreateFirewallPolicy, network-firewall:ListRuleGroups] — i.e. omitting UpdateFirewallPolicy — is ACCEPTED with HTTP 200 and an empty body. The requirement is containment (every granted action must be in the type's permitted set), not equality.
Emulator: HTTP 400 with __type InvalidResourcePolicyException and a message of the form 'The supplied policy does not match RAM managed permissions. Unexpected actions: []; missing actions: [network-firewall:UpdateFirewallPolicy].' — the validator treats a missing permitted action as a breach exactly like an unpermitted one.
Why: a client that shares a rule group with the least privilege it actually needs — the normal case, and the case RAM itself produces for a partial grant — is refused outright, so no resource policy can be attached unless the caller happens to grant every action the type admits. Code written against AWS fails on a 400 at the share step.
Fix: the policy validator in the service's state module — the action comparison must reject only actions OUTSIDE the permitted set and drop the missing-action arm; the PutResourcePolicy handler's call into it is otherwise correct.
Traces: network-firewall-resource-policy-lifecycle#task#3.

Cause 2 of 3: DescribeResourcePolicy answers 400 ResourceNotFoundException instead of 200 with the attached Policy.
AWS: DescribeResourcePolicy returns DescribeResourcePolicyResponse$Policy (PolicyString) for a resource that carries a policy; the recording shows HTTP 200 whose Policy member is the exact document the preceding PutResourcePolicy sent (a Version/Statement JSON document naming the account principal, the two granted actions and the rule-group ARN as Resource). ResourceNotFoundException ("Unable to locate a resource using the parameters that you provided") is for a resource with no policy.
Emulator: HTTP 400 with __type ResourceNotFoundException; $.Policy is absent (expected the stored document, got None).
Why: a caller cannot read back the policy it just attached, so any share-then-verify flow — and any terraform/CDK style read-after-write reconciliation — fails.
Fix: no separate defect is proven here — the describe handler returns the stored document when one is attached, so this step is expected to pass once cause 1 stops refusing the attach (Hypothesis: it is purely downstream). It is listed as its own cause because it is its own replay cluster and MUST be re-replayed, not assumed, after cause 1 is fixed; if it still answers 400 the describe path has an independent gap.
Traces: network-firewall-resource-policy-lifecycle#task#4.

Cause 3 of 3: DeleteResourcePolicy answers 400 ResourceNotFoundException instead of 200.
AWS: DeleteResourcePolicy models DeleteResourcePolicyResponse with no members and the errors InvalidResourcePolicyException and ResourceNotFoundException; the recording shows HTTP 200 with an empty body when the named resource's policy is detached.
Emulator: HTTP 400 with __type ResourceNotFoundException and a message naming the rule-group ARN (expected 200 and an empty body {}).
Why: the detach half of the lifecycle is unreachable, so a test that shares a resource and then unshares it cannot complete and leaves the caller unable to tell a failed detach from a resource that never had a policy.
Fix: as cause 2 — expected to be downstream of cause 1 (nothing was ever attached to delete). Re-replay this step after fixing cause 1 rather than closing it unrun.
Traces: network-firewall-resource-policy-lifecycle#task#5.

Relation to existing issues: the only open network-firewall issue, "Network Firewall responses omit default EncryptionConfiguration.KeyId and policy capacity counters", is about members missing from already-successful 200 responses and does not cover any status-level refusal on the resource-policy path.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200). Fixture network-firewall-resource-policy-lifecycle, which creates a stateful rule group (task#1, 200) and then drives the policy lifecycle:

Cause 1 — step task#3, PutResourcePolicy, expected 200 / actual 400:
  $: value_mismatch (expected {}, got {'__type': 'InvalidResourcePolicyException', 'Message': 'The supplied policy does not match RAM managed permissions. Unexpected actions: []; missing actions: [network-firewall:UpdateFirewallPolicy].'})
  The request's policy granted exactly [network-firewall:CreateFirewallPolicy, network-firewall:ListRuleGroups] on the rule-group ARN.

Cause 2 — step task#4, DescribeResourcePolicy, expected 200 / actual 400:
  status: expected 200, got 400
  $.Policy: missing (expected the document attached at task#3, got None)

Cause 3 — step task#5, DeleteResourcePolicy, expected 200 / actual 400:
  status: expected 200, got 400
  $: value_mismatch (expected {}, got {'__type': 'ResourceNotFoundException', 'Message': 'No object with the provided name or resource identifier was found: <rule-group ARN>'})

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): PutResourcePolicy (errors InvalidResourcePolicyException, ResourceNotFoundException), PutResourcePolicyRequest$Policy documentation listing the per-resource-type action sets, DescribeResourcePolicyResponse$Policy, DeleteResourcePolicy, InvalidResourcePolicyException (client, HTTP 400, "The policy statement failed validation").
Code citations: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/state.rs:318-398 (resource_policy_breach — its own doc comment states "SET EQUALITY, not containment, is the rule here" and it computes a `missing` list from the permitted actions), state.rs:269-302 (RULE_GROUP_POLICY_ACTIONS / permitted_actions), src/ops/put_resource_policy.rs:186-197 (handler turns any breach into InvalidResourcePolicyException before anything is attached), src/ops/describe_resource_policy.rs:160-177 (ResourceNotFoundException when no policy is attached, otherwise returns the stored document).
State reset failures in this run: 0, so these steps ran against their own fixture's state.

## Steps to Reproduce
1. aws network-firewall create-rule-group --rule-group-name rg1 --type STATEFUL --capacity 100 --rule-group '{"RulesSource":{"RulesString":"pass tcp any any -> any any (sid:1;)"}}'  -> 200, note the returned RuleGroupArn.
2. aws network-firewall put-resource-policy --resource-arn <RuleGroupArn> --policy '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":["network-firewall:CreateFirewallPolicy","network-firewall:ListRuleGroups"],"Resource":"<RuleGroupArn>"}]}'
   Emulator: 400 InvalidResourcePolicyException ("does not match RAM managed permissions ... missing actions: [network-firewall:UpdateFirewallPolicy]"). AWS: 200, empty body.
3. aws network-firewall describe-resource-policy --resource-arn <RuleGroupArn> -> emulator 400 ResourceNotFoundException; AWS 200 with Policy equal to the document sent in step 2.
4. aws network-firewall delete-resource-policy --resource-arn <RuleGroupArn> -> emulator 400 ResourceNotFoundException; AWS 200, empty body.
5. Re-run steps 3 and 4 after fixing the validator: they must both answer 200.
