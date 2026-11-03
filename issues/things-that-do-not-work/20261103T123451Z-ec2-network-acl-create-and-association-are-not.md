Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.associationSet.item.networkAclAssociationId
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.associationSet.item.networkAclId
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.default
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.networkAclId
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.tagSet
Issue-Key: ec2|DescribeNetworkAcls|2xx->2xx|$.DescribeNetworkAclsResponse.networkAclSet.item.associationSet
Issue-Related: 20261103T071133Z-ec2-describeinstances-and-launch-template-projections-omit-modeled.md
Issue-Commit: 10d8f420d9792c6a92213c81068536eef787b058

# EC2 network ACL create and association are not reflected in DescribeNetworkAcls

> _Note: this issue was identified against the code base at commit `10d8f420d9792c6a92213c81068536eef787b058`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkAcls

## Description
Two net-new causes on the network-ACL read path; both show that a created-and-associated custom ACL is not represented in state.

Cause 1 of 2 — DescribeNetworkAcls filtered by the subnet returns the VPC's DEFAULT ACL instead of the custom ACL now associated with it, with the wrong entries and no tags.
AWS: after CreateNetworkAcl, CreateNetworkAclEntry and ReplaceNetworkAclAssociation, the subnet-filtered read returns the CUSTOM acl: networkAclId of the created ACL, default 'false', an entrySet containing the authored deny rule (ruleNumber '90', protocol '6', ruleAction 'deny', egress 'false', cidrBlock '0.0.0.0/0', portRange from '22' to '22') alongside the implicit ruleNumber '32767' protocol '-1' deny entries, an associationSet whose networkAclAssociationId is the id returned by the replace and whose networkAclId is the custom ACL, and the tagSet applied at create.
Emulator: returns the default ACL (default 'true', its own networkAclId), whose entrySet carries the default allow ruleNumber '100' and the egress deny '32767' instead of the authored rules, whose associationSet still names the original association id and the default ACL, and whose tagSet is empty.
Why: a client verifying that its deny rule is in force reads an ACL that allows everything — a security control reported as applied when it is not.
Fix: CreateNetworkAcl / CreateNetworkAclEntry / ReplaceNetworkAclAssociation state updates, and the DescribeNetworkAcls lookup that resolves a subnet to its current ACL.
Traces: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#9.

Cause 2 of 2 — the ACL's associationSet is rendered empty.
AWS: in AWS a subnet is ALWAYS explicitly associated with exactly one ACL, so the ACL projection carries an associationSet item {networkAclAssociationId, networkAclId, subnetId}.
Emulator: associationSet is serialized as an empty element.
Why: a client enumerating which subnets an ACL protects concludes it protects none, and cannot discover the association id it needs for the next ReplaceNetworkAclAssociation.
Fix: the DescribeNetworkAcls projection plus the ACL-association records in state.
Traces: ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#10.

## Evidence
Cause 1: $.DescribeNetworkAclsResponse.networkAclSet.item.networkAclId value_mismatch (expected the created acl id, got the default acl id); .default value_mismatch (expected 'false', got 'true'); .entrySet.item[1] missing (expected {'ruleNumber': '90', 'protocol': '6', 'ruleAction': 'deny', 'egress': 'false', 'cidrBlock': '0.0.0.0/0', 'portRange': {'from': '22', 'to': '22'}}); .entrySet.item[0] extra (expected the '32767' deny entry, got a ruleNumber '100' ruleAction 'allow' entry); .associationSet.item.networkAclAssociationId and .networkAclId both value_mismatch; .tagSet type_mismatch (expected {'item': {'key': 'Name', 'value': ...}}, got '').
Cause 2: $.DescribeNetworkAclsResponse.networkAclSet.item.associationSet type_mismatch (expected {'item': {'networkAclAssociationId': ..., 'networkAclId': ..., 'subnetId': ...}}, got '').

## Steps to Reproduce
Replay the wire corpus (simparity-wire-aws-v1) and inspect ec2-sshp-traj-subnet-nacl-is-always-explicitly-associated#task#9 and #task#10, which create a custom NACL, add a deny entry, replace the subnet's association and then read the ACL back filtered by subnet.
