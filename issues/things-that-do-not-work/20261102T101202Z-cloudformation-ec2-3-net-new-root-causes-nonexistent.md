Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.parameter-ec2-typed-nonexistent-ids-roll-back cloudformation__ec2.parameter-ec2-typed-values-drive-created-resources cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain
Issue-Key: cloudformation__ec2.parameter-ec2-typed-nonexistent-ids-roll-back|run#13|AssertionError|assert final["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ec2.parameter-ec2-typed-values-drive-created-resources|run#24|AssertionError|assert rules == [("tcp", _PORT, _PORT, [peer])], f"ingress on the stack's group: {rules!r}"
Issue-Key: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain|run#29|Timeout|time.sleep(2)
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/ec2: 3 net-new root causes — nonexistent EC2-typed parameter ids accepted, template security-group ingress not applied, volume DeletionPolicy check never settles

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three distinct net-new root causes for EC2 resources provisioned through CloudFormation. Open issues 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md and 20261024T015148Z-cloudformation-stack-outputs-are-unusable-and-create-complete.md cover neighbouring checks and carry none of the keys below.

Cause 1 of 3: AWS-specific parameter types are not resolved, so nonexistent ids are accepted.
AWS: recorded behavior only — a parameter typed AWS::EC2::VPC::Id (and the other AWS-specific types) is validated against the account's resources; a nonexistent VpcId fails the operation and the stack rolls back to ROLLBACK_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE with no StackStatusReason for a VpcId that does not exist.
Why: the type-safety a template gets from AWS-specific parameter types is gone, and resources are 'created' against ids that do not exist.
Fix: AWS-specific parameter-type resolution in the CreateStack/UpdateStack validation stage.
Traces: cloudformation__ec2.parameter-ec2-typed-nonexistent-ids-roll-back.

Cause 2 of 3: security-group ingress declared in the template is not applied to the created group.
AWS: recorded behavior only — a stack whose AWS::EC2::SecurityGroup declares SecurityGroupIngress produces a group whose DescribeSecurityGroups IpPermissions contain exactly those rules (driven by the EC2-typed parameter values).
Emulator: the group's ingress is empty ([]).
Why: a deployed security group does not carry the rules its template declares, so connectivity behaves nothing like the template describes.
Fix: the AWS::EC2::SecurityGroup resource handler (ingress/egress property application).
Traces: cloudformation__ec2.parameter-ec2-typed-values-drive-created-resources.

Cause 3 of 3: the volume DeletionPolicy snapshot/retain lifecycle never settles.
AWS: recorded behavior only — the stack create/delete cycle with DeletionPolicy Snapshot vs Retain on AWS::EC2::Volume completes well inside the check's 60s budget, leaving a snapshot for one volume and the volume itself for the other.
Emulator: the check ends with 'Check timed out after 60.0s' — the operation never reaches a terminal state. Hypothesis: the volume create or the delete-time snapshot step never completes.
Why: a client waiting on the stack hangs, and the data-retention guarantees the policies promise are unverifiable.
Fix: the AWS::EC2::Volume provisioning/delete path (snapshot and retain handling) in the stack operation pipeline.
Traces: cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain.

## Evidence
C1: "a nonexistent VpcId must roll the stack back; got 'CREATE_COMPLETE': None".
C2: "ingress on the stack's group: []" — expected the rules the template declares from the EC2-typed parameter values.
C3: 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
C1: CreateStack with a parameter of type AWS::EC2::VPC::Id whose value does not exist and poll DescribeStacks. C2: deploy a stack with an AWS::EC2::SecurityGroup declaring SecurityGroupIngress from EC2-typed parameters and call DescribeSecurityGroups on the created group. C3: replay cloudformation__ec2.deploy-ec2-1-volume-deletion-policy-snapshot-vs-retain (two AWS::EC2::Volume resources, DeletionPolicy Snapshot and Retain, then DeleteStack).
