Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch
Issue-Key: ecr__ecs__elbv2|CreateLoadBalancer|InvalidAction
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# ECS service creation against a mismatched ELBv2 target-group type cannot be exercised: CreateLoadBalancer answers InvalidAction at version 2015-12-01

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'ecr__ecs__elbv2'. The already-open issues 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md (CreateLoadBalancer under the 'acm__elbv2' token) and 20261022T234922Z-ecr-ecs-elbv2-cloudformation-deployed-ecs-ecr-elbv2-app-stacks-are.md (CFN Outputs / missing ECS cluster) do not carry this replay key.

Cause 1 of 1: the ELBv2 awsQuery action table does not resolve CreateLoadBalancer at API version 2015-12-01.
AWS: Smithy com.amazonaws.elasticloadbalancingv2 (service version 2015-12-01, protocol awsQuery, 51 bound operations) binds CreateLoadBalancer -> {LoadBalancers} with modeled awsQuery errors DuplicateLoadBalancerName / InvalidSubnet / InvalidScheme / InvalidSecurityGroup / TooManyLoadBalancers, all HTTP 400. A well-formed Action=CreateLoadBalancer&Version=2015-12-01 POST is served 200 or refused with one of those codes; 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation CreateLoadBalancer for version 2015-12-01', so the check can never reach the ECS CreateService call whose target-group type mismatch it is asserting.
Why: no load balancer can be created, so the ECS-to-ALB binding refusal (instance-type target group given to an awsvpc service) is never observed, and the SDK is told the action does not exist rather than that the server lacks it.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10.
Traces: ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch.

## Evidence
ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateLoadBalancer operation: Could not find operation CreateLoadBalancer for version 2015-12-01.
Expected: HTTP 200 with LoadBalancers, or HTTP 400 with a modeled awsQuery code.

## Steps to Reproduce
1. Boot the emulator.
2. boto3.client('elbv2').create_load_balancer(Name='sim2real-alb', Subnets=[<two subnets>], Type='application').
3. Observe Error.Code 'InvalidAction' / 'Could not find operation CreateLoadBalancer for version 2015-12-01'.
4. Re-replay ecr__ecs__elbv2.ecs-service-create-fails-with-target-group-type-mismatch.
