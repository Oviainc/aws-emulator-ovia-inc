Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cfn-ssm-maintenance-window-registers-ec2-target-positive cloudformation__ec2__ssm.cfn-stack-output-exports-ec2-instance-id-to-ssm-parameter-positive cloudformation__ec2__ssm.sdk-ec2-instance-without-ssm-agent-not-listed-negative cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive cloudformation__ec2__ssm.sdk-ssm-inventory-collects-ec2-metadata-positive cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive cloudformation__ec2__ssm.sdk-ssm-missing-sendcommand-permission-negative cloudformation__ec2__ssm.sdk-ssm-run-command-timeout-on-stopped-ec2-negative cloudformation__ec2__ssm.sdk-ssm-send-command-executes-on-ec2-instance-positive
Issue-Key: cloudformation__ec2__ssm|CreateStack|ValidationError
Issue-Key: cloudformation__ec2__ssm.cfn-stack-output-exports-ec2-instance-id-to-ssm-parameter-positive|run#9|AssertionError|assert stored.startswith("i-"), (
Issue-Key: cloudformation__ec2__ssm.sdk-ec2-instance-without-ssm-agent-not-listed-negative|_wait_ssm_online#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-command-output-retrieved-from-ec2-positive|_wait_ssm_online#4|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive|_wait_ssm_online#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-inventory-collects-ec2-metadata-positive|_wait_ssm_online#4|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive|_wait_ssm_online#5|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-missing-sendcommand-permission-negative|_wait_ssm_online#4|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-run-command-timeout-on-stopped-ec2-negative|_wait_ssm_online#4|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ec2__ssm.sdk-ssm-send-command-executes-on-ec2-instance-positive|_wait_ssm_online#4|Timeout|time.sleep(interval_s)
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# CloudFormation+SSM: AWS::SSM::MaintenanceWindowTarget unrecognised, EC2 Ref yields a logical name, and SSM managed nodes never register (8 checks hang)

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new root causes behind the surviving cloudformation__ec2__ssm failures.

Cause 1 of 3: CreateStack refuses a template declaring AWS::SSM::MaintenanceWindowTarget as an unknown resource type.
AWS: the type is supported; a stack declaring a maintenance window plus a MaintenanceWindowTarget registering an EC2 target creates successfully (recorded behavior; the type maps onto ssm RegisterTargetWithMaintenanceWindow).
Emulator: CreateStack is refused ValidationError, "Template format error: Unrecognized resource types: [AWS::SSM::MaintenanceWindowTarget]".
Why: any template using SSM maintenance windows cannot be deployed at all — the stack never exists, so nothing downstream can be exercised.
Fix: the CloudFormation resource-type registry/provisioner (add AWS::SSM::MaintenanceWindowTarget).
Traces: cloudformation__ec2__ssm.cfn-ssm-maintenance-window-registers-ec2-target-positive.

Cause 2 of 3: Ref on an AWS::EC2::Instance resolves to the resource's generated logical/physical name instead of its instance id.
AWS: an EC2 instance resource's physical id IS its instance id, so {"Ref": "Instance"} — and any stack output or SSM parameter fed from it — carries i-<17 hex>.
Emulator: the value is the stack-derived name "<stack>-Instance-<suffix>", which is not an instance id.
Why: every downstream consumer (an SSM parameter read back and passed to DescribeInstances/SendCommand, a stack output joined to EC2) is handed a string EC2 then rejects as a malformed instance id.
Fix: the CloudFormation provisioner for AWS::EC2::Instance — set the physical resource id to the created instance id.
Traces: cloudformation__ec2__ssm.cfn-stack-output-exports-ec2-instance-id-to-ssm-parameter-positive.

Cause 3 of 3: an EC2 instance never becomes an SSM managed node, so every Run Command / inventory / parameter-on-launch scenario times out.
AWS: an instance with an SSM-core instance profile registers with SSM within minutes — DescribeInstanceInformation lists it, SendCommand is accepted and its invocation reaches Success with the command's stdout, inventory collects its metadata, and a stopped instance's SendCommand is refused InvalidInstanceId (recorded behavior).
Emulator: the node never registers and no invocation ever completes, so each of the 8 checks below ends "Check timed out after 60.0s".
Why: the whole EC2+SSM surface (send-command, command output, list-invocations, inventory, encrypted-parameter-in-UserData, the negative "no agent"/"stopped instance" branches) is unusable, and a missing registration is indistinguishable from a denied call.
Fix: the SSM agent registration the EC2 instance lifecycle should produce (DescribeInstanceInformation state) and the SendCommand/invocation state machine that depends on it.
Traces: cloudformation__ec2__ssm.sdk-ec2-instance-without-ssm-agent-not-listed-negative, .sdk-ssm-command-output-retrieved-from-ec2-positive, .sdk-ssm-encrypted-parameter-used-in-ec2-launch-positive, .sdk-ssm-inventory-collects-ec2-metadata-positive, .sdk-ssm-list-command-invocations-scoped-to-ec2-instance-positive, .sdk-ssm-missing-sendcommand-permission-negative, .sdk-ssm-run-command-timeout-on-stopped-ec2-negative, .sdk-ssm-send-command-executes-on-ec2-instance-positive.

## Evidence
From the behavioral replay:
- cfn-ssm-maintenance-window-registers-ec2-target-positive [error]: botocore ClientError (ValidationError) calling CreateStack: Template format error: Unrecognized resource types: [AWS::SSM::MaintenanceWindowTarget].
- cfn-stack-output-exports-ec2-instance-id-to-ssm-parameter-positive [mismatch]: AssertionError: parameter value should be a real EC2 instance id: got 'sim2real-xsvc-stk-3a5bf47fadad-Instance-D05FCCA52F8F'.
- the 8 sdk-ssm-*/sdk-ec2-* checks listed in Cause 3 [mismatch]: Error: Check timed out after 60.0s.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks in trace_ids. Cause 1: CreateStack with a template containing AWS::SSM::MaintenanceWindow + AWS::SSM::MaintenanceWindowTarget. Cause 2: deploy a stack whose AWS::SSM::Parameter value is {"Ref": "<EC2 instance>"} and read the parameter. Cause 3: launch an instance with an SSM-core instance profile and poll DescribeInstanceInformation / SendCommand.
