Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-firewall.rule-group-referenced-by-policy-delete-rejected network-firewall.firewall-policy-update-replaces-rule-group-reference network-firewall.disassociate-last-subnet-rejected network-firewall.subnet-change-protection-blocks-associate-rejected network-firewall.logging-configuration-s3-permission-denied-rejected
Issue-Shape-Id: com.amazonaws.networkfirewall#NetworkFirewall_20201112
Issue-Key: network-firewall.rule-group-referenced-by-policy-delete-rejected|_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: network-firewall.firewall-policy-update-replaces-rule-group-reference|run#18|AssertionError|assert rg_a_desc["RuleGroupResponse"]["NumberOfAssociations"] == 1
Issue-Key: network-firewall.disassociate-last-subnet-rejected|run#5|AssertionError|assert code == "InvalidRequestException", f"expected a rejection error, got {code}"
Issue-Key: network-firewall.subnet-change-protection-blocks-associate-rejected|run#10|AssertionError|assert code == "InvalidOperationException", f"expected InvalidOperationException, got {code}"
Issue-Key: network-firewall.logging-configuration-s3-permission-denied-rejected|run#16|AssertionError|assert code == "LogDestinationPermissionException", f"expected LogDestinationPermissionException, got {code}"
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# Network Firewall does not track policy→rule-group associations, and the DisassociateSubnets / SubnetChangeProtection / UpdateLoggingConfiguration control paths are not served

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRuleGroup

## Description
FOUR net-new root causes, one service (network-firewall). Each is its own replay cluster and each must be closed before this issue is submitted: cause 1 is a state/enforcement gap that makes a destructive call succeed where AWS refuses it, causes 2-4 are control paths that answer the emulator's scaffold error instead of the modelled behaviour. The service IS serving (CreateFirewall, CreateFirewallPolicy, CreateRuleGroup, DescribeRuleGroup and DescribeFirewall all answer 200 in these same checks), so these are fidelity gaps, not a bootstrap gap.

Cause 1 of 4: a rule group that a firewall policy still references can be deleted, and NumberOfAssociations never counts policy references.
AWS: Smithy (com.amazonaws.networkfirewall#NetworkFirewall_20201112, awsJson1_0) models DeleteRuleGroup with the error InvalidOperationException (client, HTTP 400, member Message -> ErrorMessage) documented "The operation failed because it's not valid. For example, you might have tried to delete a rule group or firewall policy that's in use."; RuleGroupResponse$NumberOfAssociations (NumberOfAssociations, integer) is documented "The number of firewall policies that use this rule group." So while exactly one firewall policy names a rule group in FirewallPolicy.StatefulRuleGroupReferences, DescribeRuleGroup must report NumberOfAssociations = 1 and DeleteRuleGroup on that rule group must be refused with HTTP 400 / InvalidOperationException; once the reference is removed (by UpdateFirewallPolicy swapping the reference, or by DeleteFirewallPolicy) the count must fall to 0 and the delete must be accepted. The refusal text is recorded behavior only — the observed example is 'Unable to delete the object because it is still in use'.
Emulator: DeleteRuleGroup on the still-referenced rule group answers HTTP 200 with RuleGroupResponse.RuleGroupStatus 'DELETING', NumberOfAssociations 0, and the rule group is torn down. DescribeRuleGroup of a rule group referenced by a freshly created policy likewise reports NumberOfAssociations 0 rather than 1. The policy's rule-group references are evidently never resolved against the rule-group store, so no association is recorded in either direction.
Why: the safety interlock AWS uses to stop a referenced rule group from being removed from under a live firewall policy does not exist — a client (or an IaC destroy ordering) that relies on the 400 to sequence teardown instead silently deletes a rule group a policy still points at, leaving a dangling reference. In the other direction, a client that polls NumberOfAssociations to decide when a rule group has become safe to delete reads 0 immediately and can never distinguish 'not referenced' from 'references not tracked'.
Fix: the service's state module — the firewall-policy record must resolve its stateless/stateful rule-group references against the rule-group store so each rule group carries a live association count, the RuleGroupResponse projection must report it, and the DeleteRuleGroup handler must refuse with InvalidOperationException while that count is non-zero. This is the same unresolved-reference gap the open issue "Network Firewall responses omit default EncryptionConfiguration.KeyId and policy capacity counters" names for the Consumed*Capacity members; that issue covers only the absent capacity members on 200 responses and does not cover NumberOfAssociations or the DeleteRuleGroup refusal.
Traces: network-firewall.rule-group-referenced-by-policy-delete-rejected (delete accepted where AWS answers 400), network-firewall.firewall-policy-update-replaces-rule-group-reference (NumberOfAssociations 0 where AWS reports 1).

Cause 2 of 4: DisassociateSubnets is not served, so the last-subnet-mapping rule is unenforced.
AWS: Smithy models DisassociateSubnets (input SubnetIds required -> AzSubnets; output FirewallArn/FirewallName/SubnetMappings/UpdateToken) with the errors InvalidOperationException, InvalidRequestException, InvalidTokenException and ResourceNotFoundException (all client, HTTP 400). Removing a firewall's only remaining subnet mapping is refused with HTTP 400 / InvalidRequestException and the mapping is left intact; the recorded message example is 'availability zone subnet(s) cannot be removed as firewall must have at least one subnet mapping, parameter: [<subnet-id>]'. With a second subnet associated, the same DisassociateSubnets succeeds (200) and DescribeFirewall reports only the remaining mapping.
Emulator: the call is answered with the scaffold error — error code 'NotImplemented', message 'operation DisassociateSubnets is scaffolded but not implemented in this emulator' — so neither the refusal nor the success path exists.
Why: a client cannot shrink a firewall's subnet mappings at all, and the one guard that protects a firewall from being left with no endpoint is absent; code written against AWS sees an unmodelled error code that no SDK retry or error branch matches.
Fix: the DisassociateSubnets handler (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/disassociate_subnets.rs) plus the firewall record's subnet-mapping mutation in the state module, including the at-least-one-mapping check that raises InvalidRequestException.
Traces: network-firewall.disassociate-last-subnet-rejected.

Cause 3 of 4: AssociateSubnets does not enforce SubnetChangeProtection.
AWS: Smithy models AssociateSubnets (input SubnetMappings required) with InvalidOperationException among its client errors (HTTP 400, "The operation failed because it's not valid"). On a firewall created with SubnetChangeProtection=True, AssociateSubnets is refused with HTTP 400 / InvalidOperationException and DescribeFirewall still shows only the original mapping; after UpdateSubnetChangeProtection sets the flag False, the identical call succeeds (200) and the new mapping appears. The recorded refusal text example is 'Cannot AssociateSubnets because SubnetChangeProtection is on'.
Emulator: error code 'NotImplemented' ('operation AssociateSubnets is scaffolded but not implemented in this emulator'), so the protection flag has no effect on any call.
Why: SubnetChangeProtection is a guardrail customers set precisely so that automation cannot re-map a firewall's subnets; against the emulator the flag is inert, so a test that proves the guardrail blocks a change cannot be written, and the 400 branch clients key on never fires.
Fix: the AssociateSubnets handler (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/associate_subnets.rs) must consult the firewall record's SubnetChangeProtection flag and raise InvalidOperationException before mutating the mappings. A separate open issue already names that handler file as an unimplemented scaffold; it does not name the SubnetChangeProtection guard, which is the behaviour this cause requires and which must be re-replayed against this check after the handler is served.
Traces: network-firewall.subnet-change-protection-blocks-associate-rejected.

Cause 4 of 4: UpdateLoggingConfiguration is not served, so neither the LogDestinationPermissionException path nor the attach path exists.
AWS: Smithy models UpdateLoggingConfiguration (input FirewallArn/FirewallName + LoggingConfiguration; output echoes them) with LogDestinationPermissionException among its client errors (HTTP 400, documented "Unable to send logs to a configured logging destination."). When Network Firewall cannot attach the log-delivery policy to the caller's own S3 bucket because the bucket policy denies the caller s3:GetBucketPolicy/s3:PutBucketPolicy, the call is refused with HTTP 400 / LogDestinationPermissionException, DescribeLoggingConfiguration still reports no LogDestinationConfigs, and removing the deny lets the identical call succeed (200) with the bucket reported back. The recorded refusal text example is 'Access Denied for LogDestination: <bucket>. Please check LogDestination permission'.
Emulator: error code 'NotImplemented' ('operation UpdateLoggingConfiguration is scaffolded but not implemented in this emulator'), for both the refusal and the success case.
Why: firewall logging cannot be configured at all, and a client that distinguishes a permission problem on the destination from a bad request gets an unmodelled error code instead of LogDestinationPermissionException.
Fix: the UpdateLoggingConfiguration handler (services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/src/ops/update_logging_configuration.rs) plus the firewall record's logging-configuration state and the destination-permission check that raises LogDestinationPermissionException.
Traces: network-firewall.logging-configuration-s3-permission-denied-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the built emulator. Per-service: network-firewall 4/22 (18%), 18 failed. Generated resource names, the account id 000000000000, the region, ARNs, RuleGroupIds and timestamps below are environment-specific and are NOT part of any requirement.

Cause 1 — network-firewall.rule-group-referenced-by-policy-delete-rejected [mismatch]:
  The check creates a STATEFUL rule group (Capacity 10, one ALERT rule, sid:1), then a firewall policy whose FirewallPolicy.StatefulRuleGroupReferences names that rule group's ARN, then calls DeleteRuleGroup on the rule group.
  Expected: the call is REJECTED with HTTP 400 / InvalidOperationException.
  Actual: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'RuleGroupResponse': {'RuleGroupArn': 'arn:aws:network-firewall:us-west-2:000000000000:stateful-rulegroup/<name>', 'RuleGroupName': '<name>', 'RuleGroupId': '<uuid>', 'Type': 'STATEFUL', 'Capacity': 10, 'RuleGroupStatus': 'DELETING', 'ConsumedCapacity': 1, 'NumberOfAssociations': 0, 'EncryptionConfiguration': {'Type': 'AWS_OWNED_KMS_KEY'}, 'LastModifiedTime': ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — note NumberOfAssociations 0 on a rule group one policy references.

Cause 1 (second witness) — network-firewall.firewall-policy-update-replaces-rule-group-reference [mismatch]:
  After CreateRuleGroup(rgA) and CreateFirewallPolicy with StatefulRuleGroupReferences=[rgA], DescribeFirewallPolicy correctly reports the reference, but:
  check_network_firewall_firewall_policy_update_replaces_rule_group_reference.py line 68: assert rg_a_desc["RuleGroupResponse"]["NumberOfAssociations"] == 1 -> AssertionError (emulator reported 0).

Cause 2 — network-firewall.disassociate-last-subnet-rejected [mismatch]:
  nfw.disassociate_subnets(FirewallArn=<arn>, SubnetIds=[<the firewall's only subnet>])
  AssertionError: expected a rejection error, got NotImplemented  (expected code InvalidRequestException; emulator returned error code NotImplemented with message 'operation DisassociateSubnets is scaffolded but not implemented in this emulator').

Cause 3 — network-firewall.subnet-change-protection-blocks-associate-rejected [mismatch]:
  Firewall created with SubnetChangeProtection=True; DescribeFirewall confirms exactly one SubnetMapping; then nfw.associate_subnets(FirewallArn=<arn>, SubnetMappings=[{'SubnetId': <second subnet in another AZ>}])
  AssertionError: expected InvalidOperationException, got NotImplemented.

Cause 4 — network-firewall.logging-configuration-s3-permission-denied-rejected [mismatch]:
  A bucket in the caller's account is given a bucket policy that Denies the account principal s3:PutBucketPolicy and s3:GetBucketPolicy; then nfw.update_logging_configuration(FirewallArn=<arn>, LoggingConfiguration={'LogDestinationConfigs': [{'LogType': 'ALERT', 'LogDestinationType': 'S3', 'LogDestination': {'bucketName': <bucket>}}]})
  AssertionError: expected LogDestinationPermissionException, got NotImplemented.

Smithy consulted (model_path services/service-com-amazonaws-networkfirewall-networkfirewall-20201112/smithy-model/com-amazonaws-networkfirewall-networkfirewall-20201112.json): DeleteRuleGroup (errors InvalidOperationException/InvalidRequestException/ResourceNotFoundException/UnsupportedOperationException, output required RuleGroupResponse), RuleGroupResponse$NumberOfAssociations ("The number of firewall policies that use this rule group"), RuleGroupResponse$Capacity, DisassociateSubnets (required SubnetIds, errors InvalidOperationException/InvalidRequestException/InvalidTokenException/ResourceNotFoundException), AssociateSubnets (required SubnetMappings, errors incl. InvalidOperationException), UpdateLoggingConfiguration (errors incl. LogDestinationPermissionException), InvalidOperationException (client, HTTP 400, Message -> ErrorMessage), InvalidRequestException (client, HTTP 400), LogDestinationPermissionException (client, HTTP 400, "Unable to send logs to a configured logging destination.").

## Steps to Reproduce
Cause 1:
1. aws network-firewall create-rule-group --rule-group-name rg1 --type STATEFUL --capacity 10 --rule-group '{"RulesSource":{"StatefulRules":[{"Action":"ALERT","Header":{"Protocol":"TCP","Source":"ANY","SourcePort":"ANY","Direction":"ANY","Destination":"ANY","DestinationPort":"ANY"},"RuleOptions":[{"Keyword":"sid:1"}]}]}}'  -> note RuleGroupArn.
2. aws network-firewall create-firewall-policy --firewall-policy-name p1 --firewall-policy '{"StatelessDefaultActions":["aws:forward_to_sfe"],"StatelessFragmentDefaultActions":["aws:forward_to_sfe"],"StatefulRuleGroupReferences":[{"ResourceArn":"<RuleGroupArn>"}]}'
3. aws network-firewall describe-rule-group --rule-group-arn <RuleGroupArn> -> RuleGroupResponse.NumberOfAssociations: emulator 0, AWS 1.
4. aws network-firewall delete-rule-group --rule-group-arn <RuleGroupArn> -> emulator 200 with RuleGroupStatus DELETING; AWS 400 InvalidOperationException.
5. Positive control to re-verify after the fix: delete p1, poll describe-rule-group until NumberOfAssociations is 0, then delete-rule-group must succeed.
6. Also re-verify the UpdateFirewallPolicy path: create rgA and rgB, point p1 at rgA, update p1's StatefulRuleGroupReferences to rgB, then rgA's NumberOfAssociations must go 1 -> 0 and rgA must become deletable.

Cause 2:
1. Create a firewall with exactly one subnet mapping.
2. aws network-firewall disassociate-subnets --firewall-arn <arn> --subnet-ids <that subnet> -> emulator error code NotImplemented; AWS 400 InvalidRequestException and the mapping untouched.
3. Associate a second subnet in another AZ, then disassociate the first -> must answer 200 and DescribeFirewall must show only the second mapping.

Cause 3:
1. aws network-firewall create-firewall ... --subnet-change-protection (SubnetChangeProtection=True), one subnet mapping.
2. aws network-firewall associate-subnets --firewall-arn <arn> --subnet-mappings SubnetId=<second subnet> -> emulator error code NotImplemented; AWS 400 InvalidOperationException with the mapping unchanged.
3. aws network-firewall update-subnet-change-protection --firewall-arn <arn> --no-subnet-change-protection, then repeat step 2 -> must succeed and DescribeFirewall must show both mappings.

Cause 4:
1. Create a firewall; create an S3 bucket in the same account/region.
2. aws s3api put-bucket-policy on that bucket with a statement {"Effect":"Deny","Principal":{"AWS":"<account-id>"},"Action":["s3:PutBucketPolicy","s3:GetBucketPolicy"],"Resource":"arn:aws:s3:::<bucket>"}.
3. aws network-firewall update-logging-configuration --firewall-arn <arn> --logging-configuration '{"LogDestinationConfigs":[{"LogType":"ALERT","LogDestinationType":"S3","LogDestination":{"bucketName":"<bucket>"}}]}' -> emulator error code NotImplemented; AWS 400 LogDestinationPermissionException, and describe-logging-configuration still reports no LogDestinationConfigs.
4. aws s3api delete-bucket-policy --bucket <bucket>, repeat step 3 -> must answer 200 and describe-logging-configuration must report the bucket.
