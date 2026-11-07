Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive|_wait_ssm_online#14|Timeout|resp = ssm.describe_instance_information(Filters=[{"Key": "InstanceIds", "Values": [instance_id]}])
Issue-Commit: 47509ee38907103620e4b84df19d39bf76901d44

# cloudformation__ec2__ssm: an EC2 instance launched with an SSM-core instance profile never becomes a managed node — DescribeInstanceInformation never reports it Online, so the SecureString-in-UserData check exhausts its budget

> _Note: this issue was identified against the code base at commit `47509ee38907103620e4b84df19d39bf76901d44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: an instance that AWS would register as an SSM managed node never appears (Online) in ssm:DescribeInstanceInformation, so every Run Command flow that targets a freshly launched instance stalls until the check's time budget expires.
AWS: an EC2 instance launched with an instance profile carrying AmazonSSMManagedInstanceCore registers with Systems Manager shortly after it reaches running: ssm:DescribeInstanceInformation(Filters=[{Key:"InstanceIds",Values:[i-...]}]) returns an InstanceInformationList entry whose PingStatus is "Online". SendCommand against that instance is then accepted and GetCommandInvocation reaches Status "Success" with the command's stdout in StandardOutputContent — here the UserData-written file containing the decrypted SecureString value. (Recorded behavior; the Smithy model carries the shapes — InstanceInformation.PingStatus, GetCommandInvocation StandardOutputContent/Status — but not the registration timing.)
Emulator: the launched instance never shows up Online. The polling loop over DescribeInstanceInformation never returns an entry, so the check never reaches SendCommand and ends "Check timed out after 180.0s". The ssm:PutParameter/GetParameter SecureString assertions before it passed (WithDecryption=True returns the plaintext, WithDecryption=False does not), so the stall is at managed-node registration, not at the parameter surface.
Why: the entire EC2+SSM data plane is unreachable for anything that launches its own instance — run-command, command output retrieval, inventory, and the negative branches that must distinguish "no agent" or "stopped instance" from a denied call. A client cannot tell a never-registering node from a slow one, so it waits forever instead of receiving a verdict.
Fix: the SSM managed-node registration the EC2 instance lifecycle must produce (the DescribeInstanceInformation state fed by a running instance with an SSM-core instance profile) and the SendCommand/GetCommandInvocation state machine that depends on it.
Traces: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive. Open issue 20261107T075601Z-cloudformation-ec2-ssm-cloudformation-ssm-aws-ssm-maintenancewindowtarget-unrecognised-ec2-ref.md names this same check in its Cause 3, but its replay key for it no longer matches (the check now exhausts a 180 s budget; the key was recorded at 60 s) and the replay report therefore lists this failure as neither ALREADY FILED nor OWNED — it is re-filed here so the failure is not dropped; a fixer closing that issue's Cause 3 closes this one with it.

## Evidence
From the behavioral replay — cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive [mismatch]:
  Error: Check timed out after 180.0s

The check: PutParameter(Type=SecureString) -> GetParameter(WithDecryption=True) must return the secret and WithDecryption=False must not (both passed) -> create an IAM role trusting ec2.amazonaws.com with AmazonSSMManagedInstanceCore and an instance profile -> RunInstances with UserData writing the secret to /tmp/decrypted-secret.txt -> wait for the instance in ssm:DescribeInstanceInformation with PingStatus == "Online" (polled up to 300 s) -> SendCommand AWS-RunShellScript "cat /tmp/decrypted-secret.txt" -> GetCommandInvocation must reach Status "Success" with the secret as StandardOutputContent.
Expected (AWS): DescribeInstanceInformation lists the instance Online within the budget and the invocation reaches Success. Actual (emulator): the DescribeInstanceInformation poll never yields an Online entry, so the check never progresses past it and is killed at its 180 s budget.

## Steps to Reproduce
Replay the behavioral suite and re-run cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive. Manually: create an IAM role trusting ec2.amazonaws.com, attach arn:aws:iam::aws:policy/AmazonSSMManagedInstanceCore, wrap it in an instance profile; RunInstances (t3.micro, AL2023 AMI from the public SSM AMI parameter, that instance profile, UserData writing a file); wait for instance_running; then poll ssm:DescribeInstanceInformation(Filters=[{Key:"InstanceIds",Values:[<id>]}]) for PingStatus "Online" and, once Online, ssm:SendCommand(AWS-RunShellScript, cat the file) + GetCommandInvocation.
