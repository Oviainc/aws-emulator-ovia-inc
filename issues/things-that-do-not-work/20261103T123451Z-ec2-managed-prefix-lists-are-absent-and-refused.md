Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters ec2-sshp-traj-referenced-group-source-has-no-cidr
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeManagedPrefixLists|2xx->2xx|$.DescribeManagedPrefixListsResponse.prefixListSet
Issue-Key: ec2|DescribeSecurityGroupRules|2xx->2xx|$.DescribeSecurityGroupRulesResponse.securityGroupRuleSet.item
Issue-Key: ec2|DescribeSecurityGroups|2xx->2xx|$.DescribeSecurityGroupsResponse.securityGroupInfo.item.ipPermissions.item.prefixListIds.item
Issue-Key: ec2|DescribeSecurityGroups|2xx->2xx|$.DescribeSecurityGroupsResponse.securityGroupInfo.item.ipPermissions.item.groups.item
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 managed prefix lists are absent and refused security-group rules persist

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeManagedPrefixLists

## Description
Four net-new causes around prefix-list sources of security-group rules.

Cause 1 of 4 — DescribeManagedPrefixLists returns an empty set for the AWS-managed prefix list.
AWS: the AWS-managed lists exist in every region and are returned by the name filter with ownerId 'AWS', prefixListName 'com.amazonaws.<region>.ec2-instance-connect', addressFamily 'IPv4', state 'create-complete', a prefixListId and the matching prefixListArn 'arn:aws:ec2:<region>:aws:prefix-list/<prefixListId>' (note the literal account segment 'aws'), and an empty tagSet.
Emulator: prefixListSet is empty, so no AWS-managed prefix list exists at all.
Why: the documented way to allow EC2 Instance Connect (or any AWS service range) into a security group cannot be expressed, and every lookup of a managed list fails.
Fix: the managed-prefix-list catalog in state plus the DescribeManagedPrefixLists lookup.
Traces: ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership#task#3.

Cause 2 of 4 — DescribeSecurityGroupRules omits the ingress rule whose source is a prefix list.
AWS: the group's rule set contains both the default egress rule and the authored ingress rule carrying prefixListId, ipProtocol 'tcp', fromPort/toPort '22', isEgress 'false', its own securityGroupRuleId and the matching securityGroupRuleArn.
Emulator: only the egress rule is returned (serialized as a single structure rather than a two-element list).
Why: a client auditing ingress rules by id sees none and cannot revoke or re-tag the rule it created.
Fix: the DescribeSecurityGroupRules lookup/serializer plus the rule record written by AuthorizeSecurityGroupIngress for a prefix-list source (follows from Cause 1 for this fixture).
Traces: ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership#task#8.

Cause 3 of 4 — DescribeSecurityGroups still lists a prefix-list source AWS refused.
AWS: the authorize that named a nonexistent prefix list is rejected, so the permission's prefixListIds holds exactly ONE entry — the valid list.
Emulator: holds two entries, the nonexistent id and the valid one.
Why: the group's effective source set is wider than anything AWS would have accepted, and the client's rule inventory contains an id that resolves to nothing.
Fix: the AuthorizeSecurityGroupIngress validation gate (see the separate refusal issue) plus the rule set written to state.
Traces: ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters#task#8 (cluster also covers task#9 of the same fixture).

Cause 4 of 4 — DescribeSecurityGroups still lists a referenced-group source AWS refused.
AWS: the permission's groups member holds exactly ONE {userId, groupId} pair, the group that exists.
Emulator: holds two pairs, including the group from the refused authorize.
Why: same as Cause 3 for group-to-group references — a rule that AWS never created is reported as active.
Fix: same gate and rule-set write path.
Traces: ec2-sshp-traj-referenced-group-source-has-no-cidr#task#8 (cluster also covers task#9 of the same fixture).

## Evidence
Cause 1: $.DescribeManagedPrefixListsResponse.prefixListSet type_mismatch — expected {'item': {'addressFamily': 'IPv4', 'ownerId': 'AWS', 'prefixListArn': 'arn:aws:ec2:<region>:aws:prefix-list/<pl-id>', 'prefixListId': '<pl-id>', 'prefixListName': 'com.amazonaws.<region>.ec2-instance-connect', 'state': 'create-complete', 'tagSet': ''}}, got ''.
Cause 2: $.DescribeSecurityGroupRulesResponse.securityGroupRuleSet.item type_mismatch — expected a 2-element list (the egress '-1' rule and an ingress rule with prefixListId, ipProtocol 'tcp', fromPort/toPort '22'), got the single egress rule structure.
Cause 3: $.DescribeSecurityGroupsResponse.securityGroupInfo.item.ipPermissions.item.prefixListIds.item type_mismatch — expected {'prefixListId': '<valid-pl>'}, got [{'prefixListId': '<nonexistent-pl>'}, {'prefixListId': '<valid-pl>'}].
Cause 4: ...ipPermissions.item.groups.item type_mismatch — expected {'userId': '<acct>', 'groupId': '<valid-sg>'}, got two pairs including the refused group.

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-sshp-traj-eic-managed-prefix-list-lookup-and-ownership#task#3 and #task#8, ec2-sshp-traj-prefix-list-source-is-opaque-to-cidr-filters#task#8, and ec2-sshp-traj-referenced-group-source-has-no-cidr#task#8.
