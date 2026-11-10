Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-missing-sendcommand-permission-negative
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-missing-sendcommand-permission-negative|_wait_ssm_online#4|Timeout|resp = ssm.describe_instance_information(Filters=[{"Key": "InstanceIds", "Values": [instance_id]}])
Issue-Commit: 97e87117743d236dfa23fd76b444a194861aeef0

# cloudformation__ec2__ssm: SDK-launched EC2 instance with an SSM instance profile never becomes usable, so the ssm:SendCommand explicit-deny refusal is never reached (check hits the harness cap)

> _Note: this issue was identified against the code base at commit `97e87117743d236dfa23fd76b444a194861aeef0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
One net-new root cause under the '### cloudformation__ec2__ssm' heading. It is NOT covered by an open issue: the nearest, 20261108T120914Z-cloudformation-ec2-ssm-cfn-deployed-ec2-ssm-estates.md, carries the replay keys of 17 OTHER checks (all CloudFormation-deployed estates) and does not carry this check's key; 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md owns only the MaintenanceWindowTarget/other sdk-ssm checks. This check is the SDK-only (no CloudFormation stack) shape of the same estate: it launches the instance directly with RunInstances + an IAM instance profile, so it exercises the plain EC2->SSM registration path and the IAM refusal of ssm:SendCommand.

Cause 1 of 1: an instance launched through the SDK with an instance profile granting AmazonSSMManagedInstanceCore never reaches a state where SSM reads about it settle, so the scenario never gets as far as asserting the ssm:SendCommand authorization refusal.

AWS: RunInstances with an IAM instance profile whose role has AmazonSSMManagedInstanceCore produces a managed node whose SSM Agent registers; ssm:DescribeInstanceInformation then lists exactly that instance with PingStatus 'Online' and ResourceType 'EC2Instance' (recorded behavior only for the registration latency — the SSM model states the shapes, not the timing; the check allows 300 s for it). ssm:ListCommands(InstanceId=<id>) on a fresh instance returns an empty Commands list (Smithy: ListCommands output Commands: CommandList, errors InvalidInstanceId/InvalidFilterKey/InvalidCommandId/InvalidNextToken/InternalServerError). A caller whose identity policy carries an explicit deny on ssm:SendCommand is refused with HTTP 400 and error code 'AccessDeniedException' and the message 'User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: ssm:SendCommand on resource: arn:aws:ec2:<region>:<account-id>:instance/<instance-id> with an explicit deny in an identity-based policy' — the fixed text is 'User: ', ' is not authorized to perform: ssm:SendCommand on resource: ' and ' with an explicit deny in an identity-based policy'; the assumed-role ARN, region, account id and instance id are values taken from the request and the caller's session, never literals from one run. Smithy: SendCommand models no AccessDeniedException (authorization refusal is the service-common IAM refusal), so the code and message are recorded behavior. The refusal must also leave NO Command record: a subsequent ListCommands for that instance still returns an empty list. An unrestricted caller's SendCommand on the same instance then reaches invocation Status 'Success'.

Emulator: the check reaches no verdict at all — the replay ends with 'Error: Check timed out after 180.0s'. No assertion is executed, so the hang is not narrowed to one operation by the report; the first blocking step of the scenario is the SSM-online wait (_wait_ssm_online, 300 s budget) that follows the direct RunInstances launch, and every sibling check under this heading that launches an SSM-profiled instance ends with the same 180 s cap. Hypothesis: the SDK-launched instance never registers as an SSM managed node (DescribeInstanceInformation never reports PingStatus 'Online' for it), so the poll loop runs past the harness cap and the SendCommand deny path, the no-Command-record effect check and the positive control are never exercised.

Why: a client that launches an SSM-managed node with RunInstances plus an instance profile (the ordinary non-CloudFormation path) can never use it: the node never appears Online, so SendCommand, its IAM refusal and command history are all unreachable, and the failure is a hang rather than a refusal, so callers and CI block instead of erroring. The authorization behavior of SendCommand is left completely unverified.

Fix: the SSM managed-node registration state that an instance-profile-carrying instance must trigger (the EC2 RunInstances/instance-profile association path feeding SSM's instance-information state), and the SendCommand handler's authorization check that must then produce the AccessDeniedException refusal without creating a Command record.

Traces: cloudformation__ec2__ssm.sdk-ssm-missing-sendcommand-permission-negative.

## Evidence
Expected: after RunInstances with an SSM-core instance profile, ssm:DescribeInstanceInformation lists the instance with PingStatus='Online' within the check's 300 s budget; ssm:ListCommands(InstanceId=<id>) returns []; a caller with an explicit deny on ssm:SendCommand gets HTTP 400 'AccessDeniedException' with message 'User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: ssm:SendCommand on resource: arn:aws:ec2:<region>:<account-id>:instance/<instance-id> with an explicit deny in an identity-based policy'; ListCommands still returns [] afterwards; the unrestricted caller's command reaches Status='Success'.
Actual: 'Error: Check timed out after 180.0s' — the check produced no response verdict; nothing after the post-launch SSM-online wait ran. The 180 s cap is this run's harness budget, not a requirement; the requirement is that the launched node becomes an Online managed node and that SendCommand then refuses as above.

## Steps to Reproduce
1. Replay the behavioral suite and read the failure under '### cloudformation__ec2__ssm' for check 'sdk-ssm-missing-sendcommand-permission-negative'.
2. Create an IAM role with AmazonSSMManagedInstanceCore plus an instance profile, resolve the AL2023 AMI from /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, and RunInstances with that profile (no CloudFormation stack involved).
3. Poll ssm:DescribeInstanceInformation for that instance id until PingStatus='Online' — it never arrives, and the check exhausts the harness cap here.
4. With the node Online, call ssm:SendCommand (DocumentName='AWS-RunShellScript') as a principal whose identity policy explicitly denies ssm:SendCommand and compare the error code and message to the AWS wording above; then call ssm:ListCommands(InstanceId=<id>) and confirm no Command was recorded; finally send the same command as an unrestricted caller and confirm the invocation reaches Success.
