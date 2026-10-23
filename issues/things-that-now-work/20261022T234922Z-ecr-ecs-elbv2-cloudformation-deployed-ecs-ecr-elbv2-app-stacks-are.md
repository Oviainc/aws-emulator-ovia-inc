Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding
Issue-Key: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing|_deploy_cfn#8|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change|_deploy_cfn#9|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: ecr__ecs__elbv2|ListTasks|ClusterNotFoundException
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CloudFormation-deployed ECS/ECR/ELBv2 app stacks are unusable: stack Outputs carry no OutputValue and the declared ECS cluster is never created (2 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
This issue bundles 2 distinct root causes; both must be closed.

Cause 1 of 2: DescribeStacks returns Outputs entries without an OutputValue member.
AWS: recorded behavior only. For a CREATE_COMPLETE stack whose template declares Outputs (here TargetGroupArn: {Ref: TargetGroup}, LoadBalancerArn: {Ref: LoadBalancer}, LoadBalancerDnsName: {Fn::GetAtt: LoadBalancer.DNSName}), each entry in Stacks[0].Outputs carries both OutputKey and the RESOLVED OutputValue (the physical resource id / attribute).
Emulator: the Outputs entries exist but have no OutputValue key at all, so the standard {o["OutputKey"]: o["OutputValue"]} read raises KeyError: 'OutputValue' and no output value is obtainable.
Why: every CFN-driven workflow reads the stack's outputs to find the resources it just deployed (target group ARN, load-balancer DNS name); with no values the deployment is unusable even though the stack reports success. An equivalent gap is filed for other services (20261022T202249Z-autoscaling-cloudformation-ec2-..., 20261022T043627Z-fis-...); neither carries these checks.
Fix: resolve Ref / Fn::GetAtt for declared Outputs when the stack is created and serialise OutputValue in the DescribeStacks response.
Traces: ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing, ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change.

Cause 2 of 2: the AWS::ECS::Cluster a completed stack declares does not exist in ECS.
AWS: recorded behavior only. After the stack reaches CREATE_COMPLETE, ecs ListTasks(cluster=<the stack's cluster>) succeeds (returning the service's tasks, possibly an empty list).
Emulator: ListTasks answers ClusterNotFoundException (HTTP 400) "Cluster not found.", so CloudFormation reported success without creating the cluster.
Why: the operator believes the platform stack is up; the first ECS read against it fails with a not-found that points at the caller rather than at the stack, and the negative assertion the check exists to grade (a service with no LoadBalancers binding) can never be evaluated.
Fix: the CloudFormation resource provisioner must materialise AWS::ECS::Cluster (and the service/task resources that reference it) into ECS state, and fail the stack when it cannot.
Traces: ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding.

## Evidence
Cause 1 — after the deploy helper waited for the stack: outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])} -> KeyError: 'OutputValue' (both checks; templates declare TargetGroupArn/LoadBalancerArn/LoadBalancerDnsName).
Cause 2 — botocore.errorfactory.ClusterNotFoundException: An error occurred (ClusterNotFoundException) when calling the ListTasks operation: Cluster not found. (expected: a successful ListTasks on the cluster the stack declared).

## Steps to Reproduce
Cause 1: CreateStack with a template declaring Outputs using {Ref: ...} and {Fn::GetAtt: ...}; wait for CREATE_COMPLETE; call DescribeStacks and inspect Stacks[0].Outputs for OutputValue.
Cause 2: CreateStack with a template declaring AWS::ECS::Cluster (plus service/target group); wait for CREATE_COMPLETE; call ecs ListTasks(cluster=<declared cluster name>) -> observe ClusterNotFoundException.
