Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-inventory-collects-ec2-metadata-positive
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-inventory-collects-ec2-metadata-positive|_wait_ssm_online#4|Timeout|resp = ssm.describe_instance_information(Filters=[{"Key": "InstanceIds", "Values": [instance_id]}])
Issue-Commit: 28a8a61701b6d4c7fe3b061a21d38872ad20994f

# cloudformation__ec2__ssm: AWS-GatherSoftwareInventory never produces an AWS:InstanceInformation entity for a real EC2 instance, so GetInventory never answers (check burns its 180 s budget)

> _Note: this issue was identified against the code base at commit `28a8a61701b6d4c7fe3b061a21d38872ad20994f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInventory

## Description
1 root cause, grounded in 1 failing check.

Cause 1 of 1: an EC2 instance launched with an AmazonSSMManagedInstanceCore instance profile never reaches a state where SSM inventory exists for it, so running the AWS-GatherSoftwareInventory association and reading it back through GetInventory never completes.

AWS: an instance whose profile role carries AmazonSSMManagedInstanceCore registers with Systems Manager and DescribeInstanceInformation reports it with PingStatus Online (Smithy: DescribeInstanceInformation -> InstanceInformationList). CreateAssociation on the AWS-managed document AWS-GatherSoftwareInventory plus StartAssociationsOnce drives the association to Status Success, after which GetInventory (Smithy: GetInventory -> Entities, Filters length 1-5, ResultAttributes length exactly 1) returns one entity per instance whose Data['AWS:InstanceInformation'].Content[0] carries InstanceId equal to that instance, PlatformType 'Linux' for an AL2023 host, ResourceType 'EC2Instance' and a non-empty IpAddress. Every step is observable within the check's 300 s per-step caps.

Emulator: the check never reaches any of those assertions — it reports 'Check timed out after 180.0s'. The first blocking wait is the poll for PingStatus Online, so on the evidence the instance never becomes a managed node and no inventory is ever collected for it. Hypothesis: the SSM managed-node registration that AWS performs for a profile-bearing instance is not modelled, so the AWS:InstanceInformation inventory type is never populated and GetInventory has nothing to serve. Open issues 20261108T203843Z-cloudformation-ec2-ssm-an-ec2-instance-with-an-ssm-core-instance.md and 20261107T133755Z-cloudformation-ec2-ssm-an-ec2-instance-launched-with.md describe the same registration gap for Run Command and DescribeInstanceInformation; neither carries this inventory check's replay key, and the inventory read path (GetInventory + the AWS-GatherSoftwareInventory association) is additional surface they do not exercise.

Why: a client that uses SSM inventory to take stock of its fleet gets nothing back for instances that really exist. Platform, resource type and IP address — the attributes AWS inventory is used for — are never queryable, and the whole association-driven inventory workflow (CreateAssociation + StartAssociationsOnce + GetInventory) silently hangs instead of returning data or an error.

Fix: the SSM managed-node registration state that an instance-profile-bearing EC2 instance should enter, and the inventory pipeline that consumes it — the AWS-GatherSoftwareInventory association execution and the GetInventory read for the AWS:InstanceInformation type.

Traces: cloudformation__ec2__ssm.sdk-ssm-inventory-collects-ec2-metadata-positive.

## Evidence
Expected (real AWS): after launching an AL2023 t3-class instance with a profile whose role has arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, DescribeInstanceInformation reports that InstanceId with PingStatus 'Online'; CreateAssociation(Name='AWS-GatherSoftwareInventory', Targets=[{Key:'InstanceIds',Values:[<id>]}]) + StartAssociationsOnce drives the association to Success; GetInventory scoped to that instance returns Entities[0].Data['AWS:InstanceInformation'].Content[0] with InstanceId == <id>, PlatformType == 'Linux', ResourceType == 'EC2Instance' and a non-empty IpAddress.

Actual (emulator): cloudformation__ec2__ssm.sdk-ssm-inventory-collects-ec2-metadata-positive ->
  Error: Check timed out after 180.0s
No assertion in the check body is reached; the run ends inside the first wait (_wait_ssm_online, 300 s cap), i.e. the instance never reports Online and no inventory entity is ever produced.

## Steps to Reproduce
1. Create an IAM role trusted by ec2.amazonaws.com, attach arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, and wrap it in an instance profile.
2. Resolve /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64 via ssm:GetParameter and RunInstances one instance with that profile into a subnet of the shared VPC.
3. Poll ssm:DescribeInstanceInformation for the instance with PingStatus 'Online'.
Observed: it never appears Online, so steps 4-5 are unreachable and the check exhausts its budget.
4. (unreachable) CreateAssociation on AWS-GatherSoftwareInventory for that instance id, then StartAssociationsOnce, and wait for association Status Success.
5. (unreachable) GetInventory scoped to that instance and read Data['AWS:InstanceInformation'].Content[0].
