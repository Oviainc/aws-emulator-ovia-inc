Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch cloudwatch__ecs__elbv2__logs.deploy-stack-missing-task-role-trust-fails-service-start cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-stack-missing-task-role-trust-fails-service-start|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx|run#6|IndexError|expected_dim_value = alb_arn.split("loadbalancer/", 1)[1]
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# CFN-deployed ECS/ALB observability stacks are unusable: stack Outputs carry no OutputValue, and the load balancer id handed back is not an ELBv2 ARN (2 causes)

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two root causes, grounded in 5 failing behavioral checks whose id's leading token is the composite 'cloudwatch__ecs__elbv2__logs'. The already-open issue 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md describes the same family of symptoms for the 'cloudformation' token and does not carry these replay keys.

Cause 1 of 2: a CREATE_COMPLETE stack exposes Output entries with no OutputValue member.
AWS: DescribeStacks returns each declared Output as an entry with OutputKey and OutputValue, OutputValue being the resolved Ref/GetAtt of the created resource. Recorded behavior only for the exact shape; the requirement is that OutputValue is present and resolved.
Emulator: the Output entry has no 'OutputValue' key — the checks raise KeyError: 'OutputValue' on their first read.
Why: the ALB DNS name, the ECS cluster/service names and the log-group name are all handed to the client through stack Outputs; without OutputValue a client cannot reach anything it deployed, so none of the ALB-routing, task-log-delivery, missing-task-role-trust or missing-log-permission behaviors can be observed.
Fix: the CloudFormation Output resolution path (stack output evaluation / DescribeStacks serializer).
Traces: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic, .deploy-ecs-task-logs-flow-to-cloudwatch, .deploy-stack-missing-task-role-trust-fails-service-start, .deploy-stack-without-log-permission-no-log-group-created.

Cause 2 of 2: the identifier CloudFormation reports for an AWS::ElasticLoadBalancingV2::LoadBalancer is not an ELBv2 ARN, so the CloudWatch dimension value for an ALB alarm cannot be derived from it.
AWS: an ALB's CloudFormation Ref/physical resource id is its ARN, of the form arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/app/<name>/<id>; the CloudWatch AWS/ApplicationELB 'LoadBalancer' dimension value is the substring after 'loadbalancer/'. Recorded behavior only for the derivation the check performs.
Emulator: the value returned contains no 'loadbalancer/' segment at all, so splitting on it yields a single element and the check fails with IndexError: list index out of range at alb_arn.split("loadbalancer/", 1)[1] — i.e. a synthesized name was returned where an ARN is required.
Why: any client that derives a CloudWatch dimension, an IAM resource condition or a target-group/listener association from an ALB's reported id gets an unusable value, and an ALB 5xx alarm can never be wired to the load balancer it is supposed to watch.
Fix: ARN minting for ELBv2 resources in the CloudFormation resource provider / ELBv2 state (the physical resource id and Ref of AWS::ElasticLoadBalancingV2::LoadBalancer must be the ELBv2 ARN).
Traces: cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx.

## Evidence
Cause 1 — cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic (line 26), .deploy-ecs-task-logs-flow-to-cloudwatch (line 26), .deploy-stack-missing-task-role-trust-fails-service-start (line 26), .deploy-stack-without-log-permission-no-log-group-created (line 28): KeyError: 'OutputValue'. Expected: Outputs[i] carries OutputKey and OutputValue.

Cause 2 — cloudwatch__ecs__elbv2__logs.deploy-cloudwatch-alarm-created-for-alb-5xx: File "check_cloudwatch__ecs__elbv2__logs_deploy_cloudwatch_alarm_created_for_alb_5xx.py", line 29, in run / expected_dim_value = alb_arn.split("loadbalancer/", 1)[1] / IndexError: list index out of range. Expected: the reported load balancer id is an ARN containing a 'loadbalancer/app/<name>/<id>' segment.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: deploy a CloudFormation template declaring an ECS cluster/service behind an ALB plus a CloudWatch log group, with Outputs exporting their identifiers; after CREATE_COMPLETE read describe_stacks()['Stacks'][0]['Outputs'][0]['OutputValue'] -> KeyError.
3. Cause 2: read the stack's reported load balancer identifier (Ref / physical resource id of the AWS::ElasticLoadBalancingV2::LoadBalancer) and split it on 'loadbalancer/' — the segment is absent, so no AWS/ApplicationELB LoadBalancer dimension value can be derived.
4. Re-replay the check ids in trace_ids.
