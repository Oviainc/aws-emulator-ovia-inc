Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected
Issue-Key: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle|phase#8|_PhaseError|raise _PhaseError(
Issue-Key: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected|run#6|AssertionError|assert code == "InvalidConfigurationRequest", (
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# OpenSearch/ALB diagnosis scenarios blocked: CFN stack Outputs carry no OutputValue, and a cross-VPC security group on CreateLoadBalancer is answered InvalidAction instead of InvalidConfigurationRequest (2 causes)

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Two root causes, grounded in 2 failing behavioral checks whose id's leading token is the composite 'ec2__elbv2__opensearch'. Related already-open issues that do NOT carry these replay keys: 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md and 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md.

Cause 1 of 2: the deploy phase of the lifecycle check cannot read a stack Output because the Output entry has no OutputValue member.
AWS: DescribeStacks returns each declared Output with OutputKey and a resolved OutputValue. Recorded behavior only for the exact shape; the requirement is that OutputValue is present on a CREATE_COMPLETE stack.
Emulator: the Output entry has no 'OutputValue' key, and the check aborts in phase 1 of 5 with KeyError: 'OutputValue', so the OpenSearch/Cognito/ALB timeout-diagnosis lifecycle is never exercised.
Why: a client cannot address the VPC domain, the internal ALB or the Cognito pool it just deployed, so every multi-phase diagnosis workflow stops at the first read.
Fix: the CloudFormation Output resolution path (stack output evaluation / DescribeStacks serializer).
Traces: ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle.

Cause 2 of 2: CreateLoadBalancer with a security group that belongs to a different VPC than the subnets is refused with the wrong error code, because the action is not dispatchable at all.
AWS: Smithy com.amazonaws.elasticloadbalancingv2#CreateLoadBalancer (service version 2015-12-01, protocol awsQuery) models InvalidConfigurationRequestException with awsQuery error code 'InvalidConfigurationRequest' and HTTP 400 — a security group from another VPC is refused with that code. 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation CreateLoadBalancer for version 2015-12-01'; the check reports 'CreateLoadBalancer: expected InvalidConfigurationRequest, got InvalidAction'.
Why: a client that branches on InvalidConfigurationRequest to repair its VPC wiring instead sees an action-does-not-exist error, which SDKs and IaC tools treat as a client/programming fault rather than an invalid configuration.
Fix: the awsQuery operation dispatch/version table of services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10, then the CreateLoadBalancer handler's security-group/subnet VPC consistency validation raising InvalidConfigurationRequest (400).
Traces: ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected.

## Evidence
Cause 1 — ec2__elbv2__opensearch.e2e-opensearch-cognito-alb-timeout-diagnosis-lifecycle: _PhaseError: [phase 1/5 deploy] KeyError: 'OutputValue'. Expected: Outputs[i] carries OutputKey and OutputValue.

Cause 2 — ec2__elbv2__opensearch.vpc-domain-and-internal-alb-with-a-security-group-from-another-vpc-rejected: AssertionError: CreateLoadBalancer: expected InvalidConfigurationRequest, got 'InvalidAction': 'Could not find operation CreateLoadBalancer for version 2015-12-01'. Expected per Smithy: HTTP 400 with awsQuery code 'InvalidConfigurationRequest'.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: deploy a CloudFormation template declaring a VPC OpenSearch domain, a Cognito user pool and an internal ALB with Outputs exporting their identifiers; read describe_stacks()['Stacks'][0]['Outputs'][0]['OutputValue'] -> KeyError.
3. Cause 2: boto3.client('elbv2').create_load_balancer(Name='sim2real-internal-alb', Subnets=[<two subnets of vpc A>], SecurityGroups=[<sg of vpc B>], Scheme='internal', Type='application'). Observe Error.Code 'InvalidAction' instead of HTTP 400 'InvalidConfigurationRequest'.
4. Re-replay both check ids in trace_ids.
