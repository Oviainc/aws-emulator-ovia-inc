Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-firewall-policy-error-paths
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-firewall-policy-error-paths network-firewall-resource-policy-lifecycle network-firewall-rule-group-stateful-lifecycle
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.ConsumedStatefulDomainCapacity
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.ConsumedStatefulRuleCapacity
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.ConsumedStatelessRuleCapacity
Issue-Key: network-firewall|CreateFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.EncryptionConfiguration.KeyId
Issue-Key: network-firewall|DeleteFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.ConsumedStatefulDomainCapacity
Issue-Key: network-firewall|DeleteFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.ConsumedStatefulRuleCapacity
Issue-Key: network-firewall|DeleteFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.ConsumedStatelessRuleCapacity
Issue-Key: network-firewall|DeleteFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.EncryptionConfiguration.KeyId
Issue-Key: network-firewall|DeleteFirewallPolicy|2xx->2xx|$.FirewallPolicyResponse.Tags
Issue-Key: network-firewall|CreateRuleGroup|2xx->2xx|$.RuleGroupResponse.EncryptionConfiguration.KeyId
Issue-Key: network-firewall|DescribeRuleGroup|2xx->2xx|$.RuleGroupResponse.EncryptionConfiguration.KeyId
Issue-Commit: 3e963f69e1d0897f40654d15e23b2fa6367d6b7c

# Network Firewall responses omit default EncryptionConfiguration.KeyId and policy capacity counters

> _Note: this issue was identified against the code base at commit `3e963f69e1d0897f40654d15e23b2fa6367d6b7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFirewallPolicy

## Description
Three distinct response-projection gaps in the Network Firewall emulator. Every affected step returns the right HTTP status (200) and the right top-level shape, but members AWS populates are absent, so a client that reads them gets None where AWS gives a value. The service IS serving (the same fixtures create, describe and delete policies successfully) — these are fidelity gaps in how a stored record is projected onto FirewallPolicyResponse / RuleGroupResponse, not a bootstrap gap. Three causes, all in one service.

Cause 1 of 3: EncryptionConfiguration is returned without KeyId when the resource uses the default AWS-owned key.
AWS: Smithy (com.amazonaws.networkfirewall, NetworkFirewall_20201112) models EncryptionConfiguration with required `Type` (EncryptionType) and optional `KeyId` (KeyId, length 1..2048, pattern \S); FirewallPolicyResponse$EncryptionConfiguration and RuleGroupResponse$EncryptionConfiguration both target it. The model does not state what KeyId holds for an AWS-owned key, so for that value this is recorded behavior only: every recorded response for a resource created WITHOUT an EncryptionConfiguration carries EncryptionConfiguration = {Type: "AWS_OWNED_KMS_KEY", KeyId: "AWS_OWNED_KMS_KEY"} — i.e. AWS echoes the AWS-owned key identifier in KeyId rather than leaving the member absent. The requirement is that both members are present on the default-encryption path, for firewall policies and for rule groups alike.
Emulator: the response carries EncryptionConfiguration.Type but KeyId is absent ($...EncryptionConfiguration.KeyId: missing, expected 'AWS_OWNED_KMS_KEY', got None) on CreateFirewallPolicy, DeleteFirewallPolicy, CreateRuleGroup and DescribeRuleGroup.
Why: a client that reads EncryptionConfiguration.KeyId to display or compare the key protecting a resource sees nothing, and any round-trip that feeds a described EncryptionConfiguration back into a create/update call loses the member — so a caller cannot tell a default-key resource from one with no encryption settings reported at all.
Fix: the shared encryption-configuration projection used by both record types — the function that builds the effective EncryptionConfiguration when the request named none — plus the two record-to-response projections that clone it.
Traces: network-firewall-firewall-policy-error-paths#setup#1 (CreateFirewallPolicy), network-firewall-firewall-policy-error-paths#teardown#6 (DeleteFirewallPolicy), network-firewall-resource-policy-lifecycle#task#1 (CreateRuleGroup), network-firewall-rule-group-stateful-lifecycle#task#2 and #task#7 (DescribeRuleGroup).

Cause 2 of 3: FirewallPolicyResponse never reports the consumed-capacity counters.
AWS: Smithy models FirewallPolicyResponse$ConsumedStatelessRuleCapacity ("The number of capacity units currently consumed by the policy's stateless rules"), $ConsumedStatefulRuleCapacity ("...the policy's stateful rules") and $ConsumedStatefulDomainCapacity ("The total number of domain name specifications across all domain list rule groups in the firewall policy that use the stateful-domain-rulegroup resource type"), all RuleCapacity integers. The recordings show that a policy referencing no rule groups reports all three as 0 — the members are present with a zero count, not omitted.
Emulator: all three are absent from CreateFirewallPolicy's and DeleteFirewallPolicy's FirewallPolicyResponse (expected 0, got None). The service source states the reason outright: the Consumed*Capacity members are left absent because a policy's rule-group references are not resolved against the rule-group store.
Why: these counters are how a caller decides whether another rule group still fits in a policy's capacity budget. Absent members force a client to treat the policy's capacity as unknown, and arithmetic over them (used + requested <= limit) breaks on a missing value instead of starting from 0.
Fix: the FirewallPolicyResponse projection in the service state module, which must resolve the policy's stateless/stateful rule-group references against the rule-group store and report the summed capacity — defaulting to 0 when the policy references nothing — rather than omitting the members.
Traces: network-firewall-firewall-policy-error-paths#setup#1, network-firewall-firewall-policy-error-paths#teardown#6.

Cause 3 of 3: DeleteFirewallPolicy's FirewallPolicyResponse omits Tags for an untagged policy.
AWS: recorded behavior only — the model's FirewallPolicyResponse$Tags targets TagList (length 1..200) and says nothing about an untagged resource, but the recorded DeleteFirewallPolicy response for a policy created with no Tags carries Tags = [] (an empty list), not an absent member.
Emulator: Tags is absent from the DeleteFirewallPolicy response (expected [], got None); the projection simply clones the record's stored tags, which are None when the create request named none.
Why: a client iterating response Tags without a null guard fails on the delete path even though the same code works against AWS; it also cannot distinguish "no tags" from "tags not reported".
Fix: the FirewallPolicyResponse projection in the service state module (empty-list default for Tags on the paths the recordings show it populated).
Traces: network-firewall-firewall-policy-error-paths#teardown#6.

Relation to existing issues: the open issue "Network Firewall rule-group capacity accounting/DryRun and resource-policy statement validation are not served" is a BEHAVIORAL issue about CreateRuleGroup capacity validation/DryRun and PutResourcePolicy statement validation; it does not cover any of the three response-projection gaps above, which are wire-shape omissions on already-succeeding 200 responses.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). All statuses matched; only members diverged.

Cause 1 — fixture network-firewall-firewall-policy-error-paths step setup#1, CreateFirewallPolicy, expected 200 / actual 200:
  $.FirewallPolicyResponse.EncryptionConfiguration.KeyId: missing (expected 'AWS_OWNED_KMS_KEY', got None)
  also at network-firewall-firewall-policy-error-paths#teardown#6 (DeleteFirewallPolicy), network-firewall-resource-policy-lifecycle#task#1 ($.RuleGroupResponse.EncryptionConfiguration.KeyId, CreateRuleGroup), network-firewall-rule-group-stateful-lifecycle#task#2 and #task#7 ($.RuleGroupResponse.EncryptionConfiguration.KeyId, DescribeRuleGroup).

Cause 2 — same step setup#1, expected 200 / actual 200:
  $.FirewallPolicyResponse.ConsumedStatelessRuleCapacity: missing (expected 0, got None)
  $.FirewallPolicyResponse.ConsumedStatefulRuleCapacity: missing (expected 0, got None)
  $.FirewallPolicyResponse.ConsumedStatefulDomainCapacity: missing (expected 0, got None)
  repeated at network-firewall-firewall-policy-error-paths#teardown#6 (DeleteFirewallPolicy, expected 200 / actual 200).

Cause 3 — network-firewall-firewall-policy-error-paths#teardown#6, DeleteFirewallPolicy, expected 200 / actual 200:
  $.FirewallPolicyResponse.Tags: missing (expected [], got None)

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): CreateFirewallPolicy (output UpdateToken + required FirewallPolicyResponse), DeleteFirewallPolicy, CreateRuleGroup, DescribeRuleGroup; shapes FirewallPolicyResponse (ConsumedStatelessRuleCapacity / ConsumedStatefulRuleCapacity / ConsumedStatefulDomainCapacity docs, Tags), RuleGroupResponse (EncryptionConfiguration), EncryptionConfiguration (required Type, optional KeyId length 1..2048 pattern \S).
Code citations: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/state.rs:87-94 (effective_encryption_configuration returns key_id: None), state.rs:909-925 (FirewallPolicyRecord::to_response — tags cloned straight through, Consumed*Capacity deliberately omitted with the comment that policy references are not resolved against the rule-group store), state.rs:980-996 (RuleGroupRecord::to_response).

## Steps to Reproduce
Cause 1:
1. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"]}'  (no --encryption-configuration)
2. Read FirewallPolicyResponse.EncryptionConfiguration: emulator gives {"Type":"AWS_OWNED_KMS_KEY"}; AWS gives {"Type":"AWS_OWNED_KMS_KEY","KeyId":"AWS_OWNED_KMS_KEY"}.
3. Repeat with create-rule-group / describe-rule-group (no --encryption-configuration): RuleGroupResponse.EncryptionConfiguration.KeyId is likewise absent.

Cause 2:
1. Using the policy from step 1 above, read FirewallPolicyResponse.ConsumedStatelessRuleCapacity / ConsumedStatefulRuleCapacity / ConsumedStatefulDomainCapacity: absent in the emulator, 0 against AWS.
2. aws network-firewall delete-firewall-policy --firewall-policy-name p1 — the returned FirewallPolicyResponse omits the same three members.

Cause 3:
1. Create a firewall policy with no --tags, then aws network-firewall delete-firewall-policy --firewall-policy-name p1.
2. Read FirewallPolicyResponse.Tags: absent in the emulator, [] in the recording.
