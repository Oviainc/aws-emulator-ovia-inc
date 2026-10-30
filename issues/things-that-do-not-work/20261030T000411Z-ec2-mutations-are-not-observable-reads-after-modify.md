Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-managed-prefix-list-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-managed-prefix-list-lifecycle ec2-modify-ipam-scope-contract ec2-security-group-rule-lifecycle ec2-route-server-association-propagation-routing-database ec2-traffic-mirror-filter-lifecycle ec2-t12-eni-attach-and-private-ip
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeManagedPrefixLists|2xx->2xx|$.DescribeManagedPrefixListsResponse.prefixListSet.item.state
Issue-Key: ec2|DescribeManagedPrefixLists|2xx->2xx|$.DescribeManagedPrefixListsResponse.prefixListSet.item.version
Issue-Key: ec2|DescribeIpamScopes|2xx->2xx|$.DescribeIpamScopesResponse.ipamScopeSet.item.description
Issue-Key: ec2|DescribeSecurityGroupRules|2xx->2xx|$.DescribeSecurityGroupRulesResponse.securityGroupRuleSet.item.description
Issue-Key: ec2|RevokeSecurityGroupIngress|2xx->2xx|$.RevokeSecurityGroupIngressResponse.revokedSecurityGroupRuleSet.item.description
Issue-Key: ec2|RevokeSecurityGroupIngress|2xx->2xx|$.RevokeSecurityGroupIngressResponse.revokedSecurityGroupRuleSet.item.toPort
Issue-Key: ec2|GetRouteServerAssociations|2xx->2xx|$.GetRouteServerAssociationsResponse.routeServerAssociationSet.item.state
Issue-Key: ec2|DescribeTrafficMirrorFilters|2xx->2xx|$.DescribeTrafficMirrorFiltersResponse.trafficMirrorFilterSet.item.ingressFilterRuleSet
Issue-Key: ec2|DescribeTrafficMirrorFilters|2xx->2xx|$.DescribeTrafficMirrorFiltersResponse.trafficMirrorFilterSet.item.networkServiceSet
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.publicDnsName
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.requesterId
Issue-Related: 20261029T215142Z-ec2-describe-projections-omit-modeled-members-and-seed.md
Issue-Commit: 7c2160283d3342f6e10783a5c4b2d926b7b7bab3

# EC2 mutations are not observable: reads after modify return the pre-modification value

> _Note: this issue was identified against the code base at commit `7c2160283d3342f6e10783a5c4b2d926b7b7bab3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
Modify/associate/add calls answer 200 but do not commit their change to emulator state, so the subsequent read still projects the pre-modification value (or a state that never advances past its transient value). AWS makes a successful mutation observable on the next read. This bundle carries 8 causes.

Cause 1 of 8: DescribeManagedPrefixLists after ModifyManagedPrefixList.
AWS: a successful modify bumps the prefix list version and drives state through modify-in-progress to the terminal modify-complete (Smithy: PrefixListState enum includes modify-in-progress/modify-complete; ManagedPrefixList.Version).
Emulator: state stays 'create-complete' and version stays '1' forever.
Why: a client waiting for modify-complete before using the new entries never proceeds, and reads a stale version number.
Fix: the ModifyManagedPrefixList handler's state transition and version increment (services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_managed_prefix_list.rs).
Traces: ec2-managed-prefix-list-lifecycle#task#4.

Cause 2 of 8: the same stall observed on the teardown poll of the prefix list, where the only assertion is the state value.
AWS: state is modify-complete after the modify.
Emulator: 'create-complete' after 61 poll attempts.
Why: same as Cause 1; it blocks teardown as well as the task path.
Fix: as Cause 1.
Traces: ec2-managed-prefix-list-lifecycle#teardown#5.

Cause 3 of 8: DescribeIpamScopes after ModifyIpamScope.
AWS: the scope's description member reflects the value supplied to ModifyIpamScope.
Emulator: description is absent from the ipamScopeSet item entirely.
Why: a client cannot read back the description it just set, and treats the modify as lost.
Fix: the ModifyIpamScope handler's write into IPAM scope state plus the description projection in describe_ipam_scopes.rs.
Traces: ec2-modify-ipam-scope-contract#task#3.

Cause 4 of 8: DescribeSecurityGroupRules after ModifySecurityGroupRules.
AWS: the rule's toPort and description are the modified values.
Emulator: returns the original toPort and the original description (recorded run: toPort 8080 instead of 8090, description 'probe app port' instead of 'probe app port range').
Why: a client that widens a port range and verifies it sees the old range, and may re-apply or report drift.
Fix: the ModifySecurityGroupRules handler's update of the stored rule.
Traces: ec2-security-group-rule-lifecycle#task#11.

Cause 5 of 8: RevokeSecurityGroupIngress echo of the revoked rule.
AWS: revokedSecurityGroupRuleSet echoes the rule as it currently stands, i.e. with the modified toPort and description.
Emulator: echoes the pre-modification values.
Why: the revoke receipt misreports which rule was removed, so audit/diff logic keyed on the echo is wrong.
Fix: the revoke handler's echo serializer, reading the current stored rule rather than the creation-time copy.
Traces: ec2-security-group-rule-lifecycle#task#13.

Cause 6 of 8: GetRouteServerAssociations after AssociateRouteServer.
AWS: the association state reaches the terminal 'associated'.
Emulator: stays 'associating'.
Why: a client gating propagation calls on 'associated' never proceeds.
Fix: the route-server association state transition in the associate handler/state.
Traces: ec2-route-server-association-propagation-routing-database#task#4.

Cause 7 of 8: DescribeTrafficMirrorFilters after CreateTrafficMirrorFilterRule and ModifyTrafficMirrorFilterNetworkServices.
AWS: ingressFilterRuleSet contains the created rule (with trafficMirrorFilterRuleId, ruleNumber, ruleAction, protocol, source/destination CIDR, trafficDirection and description as supplied) and networkServiceSet contains the enabled service (recorded: amazon-dns).
Emulator: both members come back empty.
Why: a client cannot confirm that the mirror rule or network service it added exists; a read-back validation fails against a filter the emulator believes is empty.
Fix: the rule/network-service writes in the traffic-mirror handlers and their projection in describe_traffic_mirror_filters.rs.
Traces: ec2-traffic-mirror-filter-lifecycle#task#6.

Cause 8 of 8: DescribeNetworkInterfaces after AssignPrivateIpAddresses.
AWS: privateIpAddressesSet lists the primary address plus every assigned secondary address (primary 'false' for the secondary), and the interface also carries requesterId and publicDnsName.
Emulator: privateIpAddressesSet holds only the primary address, and requesterId/publicDnsName are absent.
Why: a client that assigns a secondary IP and verifies it sees no such address, and address-management logic rebuilds or re-assigns.
Fix: the AssignPrivateIpAddresses handler's write into ENI state, plus the ENI projection in describe_network_interfaces.rs.
Traces: ec2-t12-eni-attach-and-private-ip#task#14.

## Evidence
Cause 1 - $.DescribeManagedPrefixListsResponse.prefixListSet.item.state: expected 'modify-complete', got 'create-complete'; .version expected '2', got '1'.
Cause 2 - same path, poll: not in ['modify-complete'] after 61 attempts, got 'create-complete'.
Cause 3 - $.DescribeIpamScopesResponse.ipamScopeSet.item.description: missing (expected the value set by ModifyIpamScope, got None).
Cause 4 - $.DescribeSecurityGroupRulesResponse.securityGroupRuleSet.item.toPort expected '8090' got '8080'; .description expected 'probe app port range' got 'probe app port'.
Cause 5 - $.RevokeSecurityGroupIngressResponse.revokedSecurityGroupRuleSet.item.toPort/.description show the same stale pair.
Cause 6 - $.GetRouteServerAssociationsResponse.routeServerAssociationSet.item.state: expected 'associated', got 'associating'.
Cause 7 - $.DescribeTrafficMirrorFiltersResponse.trafficMirrorFilterSet.item.ingressFilterRuleSet and .networkServiceSet: type_mismatch, expected populated item sets, got ''.
Cause 8 - $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.privateIpAddressesSet.item: expected 2 entries (primary + secondary), got only the primary; .requesterId and .publicDnsName missing.

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-managed-prefix-list-lifecycle#task#4 and #teardown#5, ec2-modify-ipam-scope-contract#task#3, ec2-security-group-rule-lifecycle#task#11 and #task#13, ec2-route-server-association-propagation-routing-database#task#4, ec2-traffic-mirror-filter-lifecycle#task#6, ec2-t12-eni-attach-and-private-ip#task#14. Each fixture mutates a resource, then reads it back.
