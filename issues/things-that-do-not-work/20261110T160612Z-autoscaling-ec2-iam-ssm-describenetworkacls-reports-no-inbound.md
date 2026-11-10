Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__iam__ssm.asgssh-e2e-troubleshoot-asg-ssh-connectivity-lifecycle
Issue-Key: autoscaling__ec2__iam__ssm.asgssh-e2e-troubleshoot-asg-ssh-connectivity-lifecycle|phase#10|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: b11785bf9bf8af57a00ad7e4d814b1c5f1d66117

# autoscaling__ec2__iam__ssm: DescribeNetworkAcls reports no inbound deny entry for tcp/22, so a blocked SSH path is indistinguishable from an open one

> _Note: this issue was identified against the code base at commit `b11785bf9bf8af57a00ad7e4d814b1c5f1d66117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkAcls

## Description
Cause 1 of 1: the network-ACL entry a stack declares (inbound deny tcp 22-22 at rule 50) is not reported in the ACL's Entries.

AWS:      DescribeNetworkAcls returns each ACL's `Entries`, one NetworkAclEntry per rule, carrying RuleNumber, Protocol, RuleAction ('allow'|'deny'), Egress, CidrBlock and — for tcp/udp rules — PortRange{From,To}. An entry created by CreateNetworkAclEntry (or by a CloudFormation AWS::EC2::NetworkAclEntry) with Egress=false, Protocol='6', RuleAction='deny', PortRange 22-22 at the rule number the template gives is therefore visible to the next describe. Recorded behavior for the rule number this estate uses (50).
Emulator: the diagnosis phase derives `('acl-<id>', None)` instead of `('acl-<id>', 50)`: no element of the ACL's `Entries` satisfies `not Egress and RuleAction == 'deny' and PortRange.From == 22`, so the deny rule that blocks SSH on the private subnet is invisible. The ACL id itself resolves, so the ACL exists and only its entry set (or the entries' PortRange/RuleAction/Egress members) is wrong.
Why:      a NACL deny entry is the reason a path is blocked. A client that reads Entries to explain or repair unreachable SSH sees an ACL with nothing denying, so every "why can't I reach port 22" derivation returns the wrong answer and any remediation (replace rule 50) is never attempted.
Fix:      the DescribeNetworkAcls serializer / the network-ACL entry state in EC2 — persist entries created on a non-default ACL and emit each with its RuleNumber, RuleAction, Egress and PortRange.
Traces:   autoscaling__ec2__iam__ssm.asgssh-e2e-troubleshoot-asg-ssh-connectivity-lifecycle (fails in phase 3/6 solve).

Related but not covering: the open issue 20261103T021956Z-ec2-network-acl-entries-interface-vpc-endpoints-and.md records neighbouring network-ACL-entry causes for checks under the `ec2` heading and does not carry this check's replay key.

## Evidence
Expected (AWS): the private subnet's ACL reports an inbound deny entry for tcp 22 at rule 50, so the derivation reads `(<PrivateSubnetNaclId>, 50)`.
Actual (emulator):
  File "check_autoscaling__ec2__iam__ssm_asgssh_e2e_troubleshoot_asg_ssh_connectivity_lifecycle.py", line 315, in phase
  AssertionError: [phase 3/6 solve] the derived ACL and deny rule read ('acl-00000000000000083', None)
The derivation is `denies = sorted(e["RuleNumber"] for e in entries if not e["Egress"] and e["RuleAction"] == "deny" and (e.get("PortRange") or {}).get("From") == 22)` over `describe_network_acls(NetworkAclIds=[nacl])["NetworkAcls"][0]["Entries"]`; it matched nothing (the acl id is per-run).

## Steps to Reproduce
1. Create a VPC, a subnet and a non-default network ACL associated with it.
2. CreateNetworkAclEntry(NetworkAclId=acl, RuleNumber=50, Protocol='6', RuleAction='deny', Egress=false, CidrBlock='0.0.0.0/0', PortRange={From:22,To:22}).
3. DescribeNetworkAcls(NetworkAclIds=[acl]) and look for an entry with Egress=false, RuleAction='deny', PortRange.From=22 — none is reported.
