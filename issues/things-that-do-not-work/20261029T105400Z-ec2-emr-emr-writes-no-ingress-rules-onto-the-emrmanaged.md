Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle
Issue-Key: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle|phase#8|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 6270af2faafa317884be9e4627608e6b01112bc2

# EMR writes no ingress rules onto the EmrManaged master/core security groups of a running cluster, so the groups carry only the template's own rules and nothing depends on them (1 cause)

> _Note: this issue was identified against the code base at commit `6270af2faafa317884be9e4627608e6b01112bc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
One root cause, grounded in the single surviving failing check of this suite. It must be closed before submitting.

Cause 1 of 1: a cluster launched with caller-supplied EmrManagedMasterSecurityGroup / EmrManagedSlaveSecurityGroup never gets EMR's own ingress rules added to those groups.

AWS: recorded behavior only — the Smithy model (com-amazonaws-emr-elasticmapreduce, Ec2InstanceAttributes.EmrManagedMasterSecurityGroup / .EmrManagedSlaveSecurityGroup) documents these members only as "The identifier of the Amazon EC2 security group for the master node." / for the core and task nodes, and says nothing about rule management. What the recording shows is the side effect: once a cluster reaches WAITING, the two groups the caller handed EMR carry ingress permissions BEYOND the ones the caller (here a CloudFormation template) declared — EMR authorizes the intra-cluster paths it needs onto the groups it manages. Because those EMR-authored permissions reference the managed groups, they are a live dependency: a caller that tries to delete the groups while they are present is refused (DependencyViolation), and the documented teardown sequence is to revoke the rules EMR added first. Concretely, against the real service a describe of the two managed groups after the cluster is WAITING returns at least one ingress permission that is not the single all-traffic permission whose only UserIdGroupPairs entry is the sibling managed group, and a revoke of those extra permissions revokes a non-zero count.

Emulator: the deploy and solve phases pass — the cluster reaches WAITING on INSTANCE_FLEET with emr-5.36.1 in the stack's subnet, DescribeCluster reports exactly the stack's two managed groups as EmrManagedMasterSecurityGroup/EmrManagedSlaveSecurityGroup, and the spark-defaults/fleet reads all match. But ec2:DescribeSecurityGroups on those two group ids returns ONLY the two permissions the template itself declared (one all-traffic permission per group whose single UserIdGroupPairs entry is the sibling group). There is nothing to revoke: the cleanup phase's revoke pass revokes 0 permissions and the check fails with
  AssertionError: [phase 3/4 cleanup] EMR should have added ingress rules to the managed groups of a running cluster; found none
So RunJobFlow records the group ids on the cluster but performs no AuthorizeSecurityGroupIngress against them.

Why: a client relying on AWS's behavior gets two things wrong. (1) It cannot observe or assert the rules EMR injects, so it cannot validate the network reachability its cluster actually has, nor detect drift on a group it also manages itself. (2) It cannot exercise the teardown ordering AWS forces: on AWS the managed groups are undeletable until EMR's rules are revoked (DependencyViolation), and infrastructure code / stack deletes must revoke-then-delete. On the emulator the groups delete with no such dependency, so revoke-before-delete logic is never exercised and a stack delete that would fail on AWS appears to succeed here.

Fix: the cluster-provisioning path of the EMR RunJobFlow handler (and the cluster state machine that takes the cluster to WAITING) must authorize EMR's intra-cluster ingress onto the EmrManaged master and slave groups via the EC2 security-group state, so DescribeSecurityGroups reflects them and RevokeSecurityGroupIngress can remove them; the EC2 security-group delete path must then count those permissions as a dependency.

Traces: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle (phase 3/4 "cleanup"). Not covered by any open issue: 20261022T235108Z-ec2-neighbouring-service-fidelity-gaps-reached-through-ec2-led.md carries the other two ec2__emr__iam checks (EMR ListInstances stub, missing Ec2AvailabilityZone) and says nothing about managed-group rule injection; 20261022T154846Z-ec2-emr-iam-cloudformation-network-stack-outputs-carry-synthesized-logical-names.md is about CloudFormation Outputs, and this check's stack outputs resolved correctly here.

## Evidence
Expected (AWS): after the stack reaches CREATE_COMPLETE and the cluster is WAITING, ec2:DescribeSecurityGroups(GroupIds=[PrimarySgId, CoreSgId]) returns, in addition to the template's two cross-group all-traffic permissions, one or more ingress permissions authored by EMR; revoking them returns a non-zero count, and only after that do the two groups delete.

Actual (emulator): phases 1/4 deploy and 2/4 solve PASS (cluster state WAITING, InstanceCollectionType INSTANCE_FLEET, ReleaseLabel emr-5.36.1, Ec2SubnetId == the stack's subnet, EmrManagedMasterSecurityGroup/EmrManagedSlaveSecurityGroup == the stack's PrimarySgId/CoreSgId; the derived diagnosis matches exactly, including core_type m5.2xlarge / core_mem_gib 32 / executor_instances 425). Phase 3/4 cleanup MISMATCH:

  File "check_ec2__emr_e2e_diagnose_emr_cluster_performance_issues_lifecycle.py", line 153, in phase
    raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
  AssertionError: [phase 3/4 cleanup] EMR should have added ingress rules to the managed groups of a running cluster; found none

The assertion is `added > 0` where `added` is the number of ingress permissions revoked from the two managed groups excluding the template's own all-traffic cross rule; the emulator yields 0. The group ids (sg-...), subnet id, cluster id (j-...) and run token are per-run values and are not part of the requirement — the requirement is that the count of EMR-authored ingress permissions on the two EmrManaged groups of a WAITING cluster is greater than zero, and that those permissions block deletion of the groups until revoked.

## Steps to Reproduce
1. ec2:CreateVpc + CreateSubnet, and ec2:CreateSecurityGroup twice to make a 'primary' and a 'core' group in that VPC.
2. Authorize exactly one ingress permission on each: IpProtocol "-1" with SourceSecurityGroupId set to the sibling group (the only rules the caller declares).
3. iam:CreateRole for the EMR service role and the EC2 role + instance profile.
4. emr:RunJobFlow with ReleaseLabel emr-5.36.1, instance fleets (MASTER and CORE, TargetOnDemandCapacity 1, InstanceType m5.2xlarge), Instances.Ec2SubnetIds=[the subnet], Instances.EmrManagedMasterSecurityGroup=<primary sg>, Instances.EmrManagedSlaveSecurityGroup=<core sg>, KeepJobFlowAliveWhenNoSteps=true.
5. Poll emr:DescribeCluster until Cluster.Status.State == WAITING.
6. ec2:DescribeSecurityGroups(GroupIds=[<primary sg>, <core sg>]) and list every IpPermissions entry that is NOT the single all-traffic permission whose only UserIdGroupPairs entry is the sibling group.
   Expected: at least one such EMR-authored permission on the managed groups. Observed: none — only the two permissions declared in step 2.
7. ec2:DeleteSecurityGroup on either managed group while the cluster is still WAITING. Expected on AWS: refused for the dependency EMR's rules create; observed on the emulator: no such dependency exists.

Or replay the check directly: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle.
