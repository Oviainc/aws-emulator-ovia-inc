Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags
Issue-Key: ec2__emr|ListInstances|NotImplemented
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# emr ListInstances is a scaffold stub, so an instance-fleet cluster's launched EC2 instances can never be enumerated

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
One net-new root cause.

Cause 1 of 1: emr ListInstances answers NotImplemented, so nothing can observe the EC2 instances an instance-fleet cluster launched.

AWS: ListInstances(ClusterId=<j-...>, InstanceFleetType='CORE') returns the Instances the fleet provisioned — each with Ec2InstanceId, InstanceType and status — so a client can join the cluster's fleet to real EC2 instances and check their subnet, security group and the emr tags (cluster id, instance-group-role=CORE kept even on instance-fleet clusters, plus the cluster's own tags). Smithy (services/service-com-amazonaws-emr-elasticmapreduce): ListInstances is a modelled paginated read returning ListInstancesOutput.Instances; its modelled failures are InternalServerException and InvalidRequestException. NotImplemented is not a modelled error of this operation.

Emulator: the call fails with error code NotImplemented and the message "operation ListInstances is scaffolded but not implemented in this emulator", surfaced as a botocore ClientError, so the check never reaches any assertion about the launched instance.

Why: every client that walks a cluster down to its EC2 instances — capacity checks, tag/compliance audits, bootstrap tooling, cost attribution — gets an unmodeled error it cannot classify, and the emr→ec2 join is unavailable.

Fix: implement the emr ListInstances handler (services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_instances.rs) so it enumerates the instances backing a cluster's instance groups and fleets, filtered by InstanceFleetType/InstanceGroupType, carrying the Ec2InstanceId of the instance the emulator actually launched.

Traces: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags. The suite did not mark this check as covered by a stub issue; open issues 20261102T094623Z-ec2-emr-emr-adds-no-ingress-rules-to.md and 20261104T102001Z-ec2-emr-2-net-new-root-causes-createvpcendpoint.md cover other emr causes, not this operation.

## Evidence
Expected: emr ListInstances(ClusterId=<cluster>, InstanceFleetType='CORE') returns the CORE fleet's instances (one m5.2xlarge, with an Ec2InstanceId resolvable through ec2 DescribeInstances in the requested subnet and core security group, tagged with the cluster id and instance-group-role=CORE).
Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListInstances operation: operation ListInstances is scaffolded but not implemented in this emulator — raised from the check's probe immediately after the cluster left STARTING.

## Steps to Reproduce
1. RunJobFlow a cluster with an InstanceFleets configuration (MASTER + CORE, CORE requesting m5.2xlarge) in a private subnet with an EMR-managed slave security group.
2. Poll until the cluster leaves STARTING.
3. Call emr ListInstances(ClusterId=<cluster>, InstanceFleetType='CORE').
4. Expected: the CORE fleet's instances with Ec2InstanceIds resolvable through ec2 DescribeInstances. Observed: ClientError (NotImplemented) "operation ListInstances is scaffolded but not implemented in this emulator".
