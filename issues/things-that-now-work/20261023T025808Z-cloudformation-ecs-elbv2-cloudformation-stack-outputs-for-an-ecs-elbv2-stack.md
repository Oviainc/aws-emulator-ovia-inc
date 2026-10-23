Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# CloudFormation stack Outputs for an ECS/ELBv2 stack carry no OutputValue, so the negative 'service without load balancer config' check cannot read any deployed value

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
One root cause, grounded in 1 failing behavioral check whose id's leading token is the composite 'cloudformation__ecs__elbv2'. The already-open issues 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md and 20261022T234922Z-ecr-ecs-elbv2-cloudformation-deployed-ecs-ecr-elbv2-app-stacks-are.md describe the same symptom for other service tokens and do not carry this replay key.

Cause 1 of 1: a stack that reaches CREATE_COMPLETE exposes Output entries with no OutputValue member.
AWS: DescribeStacks returns, for each declared Output, an entry carrying OutputKey and OutputValue, where OutputValue is the resolved value of the Output's Value expression (a Ref/GetAtt against the created resource). Recorded behavior only for the exact shape; the requirement is that OutputValue is present and resolved on a CREATE_COMPLETE stack.
Emulator: the Output entry has no 'OutputValue' key at all — the check raises KeyError: 'OutputValue' when reading it.
Why: every infrastructure-as-code workflow reads deployed identifiers (cluster name, service name, target group ARN, ALB DNS name) out of stack Outputs. With OutputValue absent, a client cannot address anything it just deployed, and the negative assertion that an ECS service declared without load-balancer config exposes no target-group binding cannot even be evaluated.
Fix: the CloudFormation Output resolution path (stack output evaluation / DescribeStacks serializer) in the cloudformation service crate — resolve each Output's Value and serialize OutputValue.
Traces: cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative.

## Evidence
cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative — Error: File "check_cloudformation__ecs__elbv2_cfn_ecs_service_without_load_balancer_config_negative.py", line 38, in run / KeyError: 'OutputValue'.
Expected: DescribeStacks Stacks[0].Outputs[i] contains both 'OutputKey' and 'OutputValue'. Actual: 'OutputValue' is absent.

## Steps to Reproduce
1. Boot the emulator.
2. cfn = boto3.client('cloudformation'); create_stack with a template declaring an ECS cluster/service and an Outputs section exporting one of their identifiers; wait for CREATE_COMPLETE.
3. cfn.describe_stacks(StackName=...)['Stacks'][0]['Outputs'][0]['OutputValue'] -> KeyError.
4. Re-replay cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative.
