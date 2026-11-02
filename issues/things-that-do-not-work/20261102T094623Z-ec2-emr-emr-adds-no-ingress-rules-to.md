Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle
Issue-Key: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: ebea418ed7238d7557ed4699ab6be023240b4505

# ec2__emr: EMR adds no ingress rules to the EmrManaged master/core security groups of a running cluster, so the diagnose-EMR-performance lifecycle cannot complete cleanup

> _Note: this issue was identified against the code base at commit `ebea418ed7238d7557ed4699ab6be023240b4505`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
Cause 1 of 1: a running EMR cluster's EmrManagedMasterSecurityGroup / EmrManagedSlaveSecurityGroup carry only the rules their creator declared — EMR itself writes none.

AWS: recorded behavior only (RunJobFlow's Smithy model describes Instances.EmrManagedMasterSecurityGroup / EmrManagedSlaveSecurityGroup / ServiceAccessSecurityGroup as the groups EMR manages, but states no rule set). When a cluster whose Instances name caller-created managed groups reaches a running state, EMR authorizes its own ingress permissions onto those groups — the mutual master<->core references and the service-access rules the cluster needs — in ADDITION to whatever rules the caller (or a CloudFormation template) declared. This is observable with ec2 DescribeSecurityGroups on the two managed group ids: permissions beyond the declared ones are present, and they are precisely what makes a later DeleteSecurityGroup fail with DependencyViolation until they are revoked. The emulator must add those EMR-authored permissions when the cluster launches and remove them when the cluster terminates.
Emulator: after the stack's cluster reaches its running state, DescribeSecurityGroups on the primary and core managed groups returns ONLY the template's own two all-traffic cross-group rules; a cleanup pass that revokes every non-declared ingress permission finds zero to revoke (`EMR should have added ingress rules to the managed groups of a running cluster; found none`).
Why: a client that models EMR's real security-group side effects — the standard teardown order "revoke EMR's added rules, then delete the groups", and any audit that asserts EMR's managed rules exist — behaves correctly against the emulator only by accident, and the whole class of DependencyViolation-on-delete behaviour that teardown exists for is unreproducible. A caller inspecting the groups also cannot see the connectivity EMR grants its own nodes, so network diagnostics built on those rules see an empty group.
Fix: the cluster-launch side effects of the EMR crate's RunJobFlow handler / its cluster state machine (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/run_job_flow.rs and the cluster-state progression it drives): when a cluster becomes running, authorize EMR's managed ingress permissions onto the EmrManaged master/core (and service-access) groups through the EC2 state, and revoke them on termination.
Traces: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle (phase 3/4 `cleanup`). The open issue 20261029T105400Z-ec2-emr-emr-writes-no-ingress-rules-onto-the-emrmanaged.md describes the same behaviour but carries no replay key for this check; this filing grounds it in the e2e lifecycle check so a fixer can re-replay it.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (service heading `ec2__emr`, 0/4 checks passing).

ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle: phases `deploy` and `solve` passed — the CloudFormation EMR estate reached its running state and the read-only diagnosis derived from live EMR/EC2 reads matched exactly. Phase 3/4 `cleanup` then failed: `AssertionError: [phase 3/4 cleanup] EMR should have added ingress rules to the managed groups of a running cluster; found none`.

The cleanup step enumerates ec2.describe_security_groups(GroupIds=[primary_sg, core_sg]) and revokes every ingress permission that is NOT the template's declared all-traffic cross-group rule; it counted 0 such permissions. Immediately afterwards the check asserts that only the template's two cross rules remain — which trivially holds, because EMR contributed nothing. On real AWS the same pass revokes EMR's added permissions (and without that revoke the group deletes hit DependencyViolation).

## Steps to Reproduce
Boot the emulator and replay ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle from suite simparity-behavioral-aws-v1 (service heading `ec2__emr`):
1. Deploy the `emr-spark-misconfigured` CloudFormation template: a VPC/subnet, a primary and a core security group with one all-traffic cross-group rule each, and an AWS::EMR::Cluster whose Instances name those two groups as EmrManagedMasterSecurityGroup / EmrManagedSlaveSecurityGroup; wait for the cluster to reach WAITING.
2. Call ec2 DescribeSecurityGroups on the two managed group ids and list IpPermissions.
3. Assert at least one ingress permission is present that the template did not declare (EMR's own master<->core / service-access rules), and that revoking those permissions is what allows the subsequent DeleteSecurityGroup to succeed.
