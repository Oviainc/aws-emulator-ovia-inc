Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle
Issue-Key: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# ec2__emr: a CFN stack declaring AWS::EMR::Cluster on an EC2/IAM estate rolls back — ROLLBACK_COMPLETE instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1 — CreateStack of the EMR-Spark estate (public-subnet VPC + EMR managed security groups + EMR service/EC2 roles + AWS::EMR::Cluster on instance fleets) never reaches CREATE_COMPLETE; the emulator rolls the whole stack back. Failing check: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle.

AWS: CloudFormation CreateStack on this template provisions AWS::EC2::VPC, AWS::EC2::InternetGateway, AWS::EC2::VPCGatewayAttachment, AWS::EC2::Subnet, AWS::EC2::RouteTable, AWS::EC2::Route, AWS::EC2::SubnetRouteTableAssociation, two AWS::EC2::SecurityGroup resources with reciprocal AWS::EC2::SecurityGroupIngress rules, AWS::IAM::Role x2, AWS::IAM::InstanceProfile and AWS::EMR::Cluster (ReleaseLabel emr-5.36.1, Applications Spark/Zeppelin/Hadoop/Pig, MasterInstanceFleet + CoreInstanceFleet at TargetOnDemandCapacity 1, a spark-defaults Configurations block, ScaleDownBehavior TERMINATE_AT_TASK_COMPLETION, AutoTerminationPolicy IdleTimeout 3600), and the stack settles at CREATE_COMPLETE with Outputs ClusterId, VpcId, SubnetId, PrimarySgId and CoreSgId carrying real identifiers (the EMR cluster then reaches WAITING). Smithy: com.amazonaws.cloudformation#StackStatus enum includes CREATE_COMPLETE and ROLLBACK_COMPLETE; which one a valid template yields is recorded behavior only (CloudFormation resource provisioning is not modelled in Smithy), and the recording is CREATE_COMPLETE.

Emulator: the check polls for any terminal state and the stack settles at ROLLBACK_COMPLETE — "stack sim2real-scn-emrdiag-<id>-estate reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE". No Outputs are produced, so no cluster id, VPC, subnet or security-group id exists for the scenario to work with. The stack name in the message is a per-run value; the requirement is the terminal status.

Why: a client that provisions an EMR cluster the standard way — one CloudFormation stack carrying the network, the two EMR service roles and the AWS::EMR::Cluster — gets nothing back and no cluster. Every subsequent step of the EMR scenario (read the cluster's Configurations, diagnose the oversized spark-defaults against a single core node, drive the cluster to a terminal state, delete the stack) is unreachable. Because the rollback completes, the failure is silent in the sense that the client sees a clean empty stack rather than a diagnosable resource error, and CI reads it as "deploy does not work" with no actionable resource name.

Hypothesis: the failing resource is upstream of EMR. This suite's own report marks EC2 CreateInternetGateway (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs) and CreateRouteTable (.../create_route_table.rs) as unimplemented scaffold stubs, and this template declares both, so the network leg of the stack cannot be provisioned; AWS::EMR::Cluster DependsOn DefaultRoute/SubnetRta and both SecurityGroupIngress rules, so it never starts. Those stubs have their own filed issues; what is not covered by them is this stack's terminal status and the absence of the five declared Outputs.

Fix: the CloudFormation stack-engine's resource provisioning for this template's EC2 network types plus its AWS::EMR::Cluster provisioner (the component that must turn the declared Instances/InstanceFleets/Configurations into an EMR RunJobFlow and expose the cluster id as the resource's physical id and the stack's ClusterId Output). Related open issues that do NOT cover this check: 20261027T155034Z-cloudformation-ec2-cfn-provisioned-aws-ec2-vpc-and.md and 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md.

Traces: ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle (phase 1/4 deploy).

## Evidence
Expected (AWS): cloudformation.CreateStack of deployments/emr-spark-misconfigured/template.json settles at StackStatus 'CREATE_COMPLETE' and DescribeStacks returns Outputs ClusterId, VpcId, SubnetId, PrimarySgId, CoreSgId with real identifiers; elasticmapreduce.DescribeCluster(ClusterId) then reports the cluster progressing to WAITING with the declared spark-defaults Configurations.

Actual (emulator): the stack settles at StackStatus 'ROLLBACK_COMPLETE' with no Outputs. Replay detail:
  AssertionError: [phase 1/4 deploy] stack sim2real-scn-emrdiag-7d86ace08e7d-estate reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE
(the fixed text is the check's; 'sim2real-scn-emrdiag-7d86ace08e7d-estate' is a per-run stack name and not part of the requirement. The requirement is the status value 'CREATE_COMPLETE'.)

Smithy: com.amazonaws.cloudformation#StackStatus = [..., CREATE_COMPLETE, ROLLBACK_IN_PROGRESS, ROLLBACK_FAILED, ROLLBACK_COMPLETE, ...].

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it.
2. cfn.create_stack(StackName='emr1', TemplateBody=<deployments/emr-spark-misconfigured/template.json>, Parameters=[{AzName: an AZ offering m5.2xlarge}, {RunToken: any}, {InstanceType: m5.2xlarge}, {ReleaseLabel: emr-5.36.1}], Capabilities=['CAPABILITY_IAM']).
3. Poll cfn.describe_stacks(StackName='emr1')['Stacks'][0]['StackStatus'] until terminal.
4. Observe 'ROLLBACK_COMPLETE' and an empty Outputs list; AWS reaches 'CREATE_COMPLETE' with the five declared Outputs populated.
5. Equivalently: replay check ec2__emr.e2e-diagnose-emr-cluster-performance-issues-lifecycle.
