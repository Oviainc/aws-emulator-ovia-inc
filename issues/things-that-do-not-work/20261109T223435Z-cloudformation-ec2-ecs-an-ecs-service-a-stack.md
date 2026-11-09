Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ecs.cfn-ecs-service-resource-lifecycle
Issue-Key: cloudformation__ec2__ecs.cfn-ecs-service-resource-lifecycle|run#16|AssertionError|assert not described["failures"], (
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__ec2__ecs: an ECS service a stack reports CREATE_COMPLETE is reported MISSING by DescribeServices

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Cause 1 of 1: the AWS::ECS::Service a stack creates does not exist in ECS.
AWS: after the stack reaches CREATE_COMPLETE, DescribeServices(cluster=<cluster>, services=[<service arn>]) returns the service in `services` with an empty `failures` list.
Emulator: DescribeServices returns the service under `failures` with reason `MISSING` and the service ARN the stack minted, so `services` is empty.
Why: a client that deploys an ECS service with CloudFormation and then drives it (scale, update, list tasks) through the ECS API is told the service does not exist, even though the stack says it does.
Fix: the AWS::ECS::Service resource handler must register the service in ECS state (CreateService) rather than only minting an ARN.
Traces: the failing check below.

## Evidence
AssertionError: DescribeServices reported failures: [{'arn': 'arn:aws:ecs:us-west-2:000000000000:service/<cluster>/<service>', 'reason': 'MISSING'}]. Expected the service present in `services` with no failures.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Deploy a stack with an AWS::ECS::Cluster, an AWS::ECS::TaskDefinition and an AWS::ECS::Service.
3. After CREATE_COMPLETE, call DescribeServices with the service ARN the stack reports.
