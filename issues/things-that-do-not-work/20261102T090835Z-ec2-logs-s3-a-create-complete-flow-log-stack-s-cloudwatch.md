Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__logs__s3.e2e-check-vpc-flow-log-destinations-lifecycle
Issue-Key: ec2__logs__s3.e2e-check-vpc-flow-log-destinations-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 86c9f1e4438a61fbe37b19dfcd3d77efce3d0679

# A CREATE_COMPLETE flow-log stack's CloudWatch Logs destination group is absent, so the flow-log read trajectory stops at DescribeLogGroups (1 cause)

> _Note: this issue was identified against the code base at commit `86c9f1e4438a61fbe37b19dfcd3d77efce3d0679`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLogGroups

## Description
Cause 1 of 1 — the log group a VPC-flow-log stack declares as its CloudWatch Logs destination is not described after the stack settles.
AWS: recorded behavior only. A CloudFormation stack that declares an AWS::Logs::LogGroup as the destination of an AWS::EC2::FlowLog reaches CREATE_COMPLETE only with that group in place; DescribeLogGroups scoped to the stack's own name prefix lists it, and DescribeLogStreams on it is then issued (and answers, with the flow-log delivery stream once records arrive).
Emulator: DescribeFlowLogs answers, but DescribeLogGroups returns no group carrying the stack's name token, so the solve never issues DescribeLogStreams at all — the read trajectory the check asserts, (ec2:DescribeFlowLogs, logs:DescribeLogGroups, logs:DescribeLogStreams), stops after two operations.
Why: the destination log group is the resource that makes a cloud-watch-logs flow log readable. Absent, a client cannot locate, let alone read, any flow-log data, and a stack that reported CREATE_COMPLETE has silently not provisioned the resource its flow log points at.
Fix: the AWS::Logs::LogGroup resource provisioner in the cloudformation crate's resource dispatch (the group must exist, and be visible to DescribeLogGroups by prefix, before the stack settles), together with the flow-log destination wiring in the ec2 crate. Note: open issue 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md records this family and carries none of this check's keys.
Traces: ec2__logs__s3.e2e-check-vpc-flow-log-destinations-lifecycle

One cause must be closed before submitting.

## Evidence
From the simparity-behavioral-aws-v1 replay against this checkout.

e2e-check-vpc-flow-log-destinations-lifecycle:
  AssertionError: [phase 2/4 solve] the read fired (('ec2', 'DescribeFlowLogs'), ('logs', 'DescribeLogGroups')), the asserted sequence is (('ec2', 'DescribeFlowLogs'), ('logs', 'DescribeLogGroups'), ('logs', 'DescribeLogStreams'))

The solve issues DescribeLogStreams once per log group whose name carries the stack's token; the list was empty, so the third operation was never issued.

## Steps to Reproduce
Boot the emulator from this checkout.

1. Deploy a stack declaring a VPC, an AWS::Logs::LogGroup and an AWS::EC2::FlowLog whose LogDestinationType is cloud-watch-logs and whose LogGroupName is that group; wait for CREATE_COMPLETE.
2. aws ec2 describe-flow-logs --filter Name=resource-id,Values=<vpcId>  -> answers.
3. aws logs describe-log-groups --log-group-name-prefix <stack name token>  -> no group.
AWS lists the declared group, and describe-log-streams on it then answers.

Then re-replay: ec2__logs__s3.e2e-check-vpc-flow-log-destinations-lifecycle
