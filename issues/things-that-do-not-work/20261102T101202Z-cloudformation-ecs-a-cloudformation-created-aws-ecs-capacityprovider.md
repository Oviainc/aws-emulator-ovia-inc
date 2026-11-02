Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api
Issue-Key: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api|run#31|AssertionError|assert len(described) == 1, (
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/ecs: a CloudFormation-created AWS::ECS::CapacityProvider is invisible to DescribeCapacityProviders

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCapacityProviders

## Description
Cause 1 of 1: the capacity provider a stack creates is not registered in the ECS control plane.
AWS: recorded behavior only — a stack declaring AWS::ECS::CapacityProvider (CDK's L2 AsgCapacityProvider with managed scaling) creates a provider that DescribeCapacityProviders returns, with its auto-scaling-group provider and managedScaling settings.
Emulator: DescribeCapacityProviders for the created provider name returns 0 providers, so the ECS API never sees the resource the stack reports as CREATE_COMPLETE.
Why: a client that deploys ECS capacity through CloudFormation and then drives it through the ECS API (attach to a cluster, read managed scaling) finds nothing.
Fix: the AWS::ECS::CapacityProvider resource handler must create the provider in ECS state (and return its name/ARN as the physical resource id).
Traces: cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api.

## Evidence
AssertionError: "DescribeCapacityProviders for '<stack>-CapE1A612B3-<suffix>' returned 0 providers, expected 1". The provider name is a per-run generated value.

## Steps to Reproduce
Deploy a CDK app with an ECS cluster and an AsgCapacityProvider (managed scaling enabled), then call DescribeCapacityProviders with the created provider's name — replay cloudformation__ecs.cdk-l2-ecs-asg-capacity-provider-managed-scaling-reaches-the-ecs-api.
