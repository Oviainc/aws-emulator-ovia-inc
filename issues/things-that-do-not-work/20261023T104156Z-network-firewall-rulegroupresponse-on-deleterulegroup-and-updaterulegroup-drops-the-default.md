Issue-Source: trace-sweep-divergence
Issue-Trace: network-firewall-rule-group-error-paths
Issue-Step: teardown#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: network-firewall-rule-group-error-paths network-firewall-rule-group-stateful-lifecycle
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall|DeleteRuleGroup|2xx->2xx|$.RuleGroupResponse.EncryptionConfiguration.KeyId
Issue-Key: network-firewall|DeleteRuleGroup|2xx->2xx|$.RuleGroupResponse.Tags
Issue-Key: network-firewall|UpdateRuleGroup|2xx->2xx|$.RuleGroupResponse.EncryptionConfiguration.KeyId
Issue-Commit: b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf

# RuleGroupResponse on DeleteRuleGroup and UpdateRuleGroup drops the default KeyId and Tags members

> _Note: this issue was identified against the code base at commit `b887f4c166159c5dcfc6801bcb3cb6a6bedc24bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRuleGroup

## Description
Two response-projection gaps on rule-group operations that the open issue "Network Firewall responses omit default EncryptionConfiguration.KeyId and policy capacity counters" does NOT cover: its replay keys name CreateFirewallPolicy, DeleteFirewallPolicy, CreateRuleGroup and DescribeRuleGroup, while the two clusters below are on DeleteRuleGroup and UpdateRuleGroup and carry their own fixture steps, so they are not exercised by that issue's pointer. Status is 200 on both and only members diverge. TWO causes, one service.

Cause 1 of 2: DeleteRuleGroup's RuleGroupResponse omits EncryptionConfiguration.KeyId and omits Tags for an untagged rule group.
AWS: Smithy (com.amazonaws.networkfirewall#DeleteRuleGroup, awsJson1_0) returns RuleGroupResponse, whose EncryptionConfiguration targets a structure with required Type (EncryptionType) and optional KeyId (length 1..2048, pattern \S), and whose Tags targets TagList (length 1..200). The model does not state what either holds on the default path, so this is recorded behavior only: for a rule group created with no EncryptionConfiguration and no Tags, the recorded DeleteRuleGroup response carries EncryptionConfiguration = {Type: "AWS_OWNED_KMS_KEY", KeyId: "AWS_OWNED_KMS_KEY"} and Tags = [] — both members PRESENT, the AWS-owned key echoed into KeyId and an empty list rather than an absent member.
Emulator: 200 with the right shape, but $.RuleGroupResponse.EncryptionConfiguration.KeyId is absent (expected 'AWS_OWNED_KMS_KEY', got None) and $.RuleGroupResponse.Tags is absent (expected [], got None).
Why: a client that reads the key protecting a deleted rule group sees nothing, and a client that iterates response Tags without a null guard fails on the delete path even though the same code works against AWS; neither caller can distinguish "default key / no tags" from "not reported".
Fix: the rule-group record-to-response projection in the service state module — the shared effective-encryption-configuration helper must populate KeyId on the AWS-owned-key path, and the projection must default Tags to an empty list instead of cloning a None.
Traces: network-firewall-rule-group-error-paths#teardown#6.

Cause 2 of 2: UpdateRuleGroup's RuleGroupResponse omits EncryptionConfiguration.KeyId.
AWS: UpdateRuleGroup returns the same RuleGroupResponse shape; the recorded response for a rule group using the default AWS-owned key carries EncryptionConfiguration = {Type: "AWS_OWNED_KMS_KEY", KeyId: "AWS_OWNED_KMS_KEY"} (recorded behavior only for the KeyId value, as above).
Emulator: 200, $.RuleGroupResponse.EncryptionConfiguration.KeyId absent (expected 'AWS_OWNED_KMS_KEY', got None).
Why: an update is the point at which a caller re-reads the resource's encryption settings to confirm they survived the change, and feeding the returned EncryptionConfiguration back into a later update silently drops the member.
Fix: the same shared encryption-configuration projection, reached through the UpdateRuleGroup handler's response build — fixing the helper should cover both causes, but each is its own replay cluster and both steps must be re-replayed.
Traces: network-firewall-rule-group-stateful-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200; state_reset_failures 0). Both clusters: expected status 200, actual status 200 — only members diverged.

Cause 1 — fixture network-firewall-rule-group-error-paths step teardown#6, DeleteRuleGroup:
  $.RuleGroupResponse.EncryptionConfiguration.KeyId: missing (expected 'AWS_OWNED_KMS_KEY', got None)
  $.RuleGroupResponse.Tags: missing (expected [], got None)

Cause 2 — fixture network-firewall-rule-group-stateful-lifecycle step task#6, UpdateRuleGroup:
  $.RuleGroupResponse.EncryptionConfiguration.KeyId: missing (expected 'AWS_OWNED_KMS_KEY', got None)

Neither cluster carried a stub_path or an owning issue in the replay report; the open issue 20261023T001140Z-network-firewall-responses-omit-default-encryptionconfiguration-keyid-and.md owns the CreateFirewallPolicy / DeleteFirewallPolicy / CreateRuleGroup / DescribeRuleGroup clusters of the same family and not these two.
Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): DeleteRuleGroup and UpdateRuleGroup (output RuleGroupResponse + UpdateToken), RuleGroupResponse (EncryptionConfiguration, Tags -> TagList length 1..200), EncryptionConfiguration (required Type, optional KeyId length 1..2048 pattern \S).
Code citations: services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/state.rs:81-94 (effective_encryption_configuration returns key_id: None for the default AWS-owned key), state.rs around the RuleGroupRecord-to-RuleGroupResponse projection (tags cloned straight through from the record, so None when the create request named none).

## Steps to Reproduce
Cause 1:
1. aws network-firewall create-rule-group --rule-group-name rg1 --type STATEFUL --capacity 100 --rule-group '{"RulesSource":{"RulesString":"pass tcp any any -> any any (sid:1;)"}}'   (no --encryption-configuration, no --tags)
2. aws network-firewall delete-rule-group --rule-group-name rg1 --type STATEFUL
3. Read the response: emulator gives RuleGroupResponse.EncryptionConfiguration = {"Type":"AWS_OWNED_KMS_KEY"} and no Tags member; AWS gives {"Type":"AWS_OWNED_KMS_KEY","KeyId":"AWS_OWNED_KMS_KEY"} and Tags = [].

Cause 2:
1. Create a stateful rule group as above and describe it to obtain its UpdateToken.
2. aws network-firewall update-rule-group --rule-group-name rg1 --type STATEFUL --update-token <token> --rule-group '{"RulesSource":{"RulesString":"pass tcp any any -> any any (sid:2;)"}}'
3. Read RuleGroupResponse.EncryptionConfiguration.KeyId: absent in the emulator, 'AWS_OWNED_KMS_KEY' in the recording.
