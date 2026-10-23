Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch cloudwatch__ecs__elbv2__logs.deploy-stack-missing-task-role-trust-fails-service-start cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding elbv2.deploy-truststore-cfn-bundle-integrity-behaves elbv2.deploy-cfn-alb-nonexistent-subnet-rejected elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch|_deploy_cfn#3|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-stack-missing-task-role-trust-fails-service-start|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created|_deploy_cfn#2|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: elastic-load-balancing-v2|DescribeTargetHealth|TargetGroupNotFound
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative|probe#9|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Key: ecr__ecs__elbv2|ListTasks|ServiceNotFoundException
Issue-Key: elastic-load-balancing-v2|DescribeTrustStores|TrustStoreNotFound
Issue-Key: elbv2.deploy-cfn-alb-nonexistent-subnet-rejected|run#9|AssertionError|raise AssertionError("expected the bad-subnet stack to roll back, but it reached CREATE_COMPLETE")
Issue-Key: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected|_poll_stable#18|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# CloudFormation reports CREATE_COMPLETE for resources it never created (ELBv2 target groups, trust stores, ECS services), leaves stack Outputs without OutputValue, and never rolls back on invalid/conflicting resources

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
SIX distinct root causes are bundled here; all six must be closed. Each is grounded in checks that deploy a template and then read the resources back through the owning service's API. All of them are CloudFormation-side: the stack reports success, the resource does not exist, or the stack's own Outputs are unusable.

Cause 1 of 6: stack Outputs carry an OutputKey but no OutputValue.
AWS: DescribeStacks returns each declared Output as {OutputKey, OutputValue[, Description, ExportName]} with OutputValue fully resolved (Ref/Fn::GetAtt/Fn::Sub evaluated) once the stack is CREATE_COMPLETE.
Emulator: the Outputs entries exist but have no OutputValue member, so the standard `{o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}` idiom raises KeyError: 'OutputValue' and the deployment is unusable past the deploy step.
Why: Outputs are the only contract a template exposes to its caller; every post-deploy step (cluster name, log group, task-definition ARN, subnet ids) is lost, so CFN cannot be driven programmatically at all.
Fix: output resolution in the CloudFormation stack handler/serializer (evaluate the intrinsic and emit OutputValue). Related open issues that do not carry these keys: 20261023T025808Z-ec2-elbv2-opensearch-opensearch-alb-diagnosis-scenarios-blocked-cfn-stack-outputs.md, 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md.
Traces: cloudwatch__ecs__elbv2__logs.deploy-ecs-task-logs-flow-to-cloudwatch, cloudwatch__ecs__elbv2__logs.deploy-stack-missing-task-role-trust-fails-service-start, cloudwatch__ecs__elbv2__logs.deploy-stack-without-log-permission-no-log-group-created.

Cause 2 of 6: AWS::ElasticLoadBalancingV2::TargetGroup reaches CREATE_COMPLETE but the target group does not exist in ELBv2.
AWS: once the stack is CREATE_COMPLETE, the target group's physical id is a real target-group ARN and DescribeTargetHealth/DescribeTargetGroups on it succeed.
Emulator: DescribeTargetHealth on the ARN the stack reports answers 400 TargetGroupNotFound ("Target group 'arn:aws:elasticloadbalancing:...:targetgroup/sim2real-TargetGro-.../...' not found") — the CFN-side resource and the ELBv2 store are not the same object.
Why: a stack that claims success hands the caller a dangling ARN; nothing downstream (ECS service registration, health polling, listener rules) can work, and the failure surfaces far from its cause.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::TargetGroup — it must create the resource through the elbv2 state it later reports.
Traces: autoscaling__ecs__elbv2.deploy-ecs-service-with-alb-target-group-cfn, cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic, ecr__ecs__elbv2.deploy-cfn-ecs-ecr-elbv2-stack-provisions-routing, ecr__ecs__elbv2.deploy-cfn-updates-service-on-image-tag-change.

Cause 3 of 6: AWS::ECS::Service reaches CREATE_COMPLETE but the service does not exist in ECS.
AWS: the deployed service is visible to DescribeServices and ListTasks right after CREATE_COMPLETE.
Emulator: DescribeServices returns an empty "services" list (the check's `["services"][0]` raises IndexError) and ListTasks answers ServiceNotFoundException "Service not found." for the service the same stack declared.
Why: the whole ECS-on-CFN path is a no-op that reports success — the central deployment model for container workloads.
Fix: the CloudFormation resource provider for AWS::ECS::Service (create through ECS state, and propagate its name/ARN as the physical id).
Traces: cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative, ecr__ecs__elbv2.deploy-cfn-negative-missing-service-target-group-binding.

Cause 4 of 6: AWS::ElasticLoadBalancingV2::TrustStore reaches CREATE_COMPLETE but the trust store does not exist in ELBv2.
AWS: after CREATE_COMPLETE, DescribeTrustStores finds the store by name/ARN and its CA bundle is readable.
Emulator: DescribeTrustStores answers 400 TrustStoreNotFound ("Trust store 'sim2real-ts-...' not found") for the store the stack just created.
Why: mTLS cannot be deployed as infrastructure-as-code even once the trust-store API exists.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::TrustStore.
Traces: elbv2.deploy-truststore-cfn-bundle-integrity-behaves.

Cause 5 of 6: a stack naming a nonexistent subnet reaches CREATE_COMPLETE instead of rolling back.
AWS: the AWS::ElasticLoadBalancingV2::LoadBalancer create fails (InvalidSubnet/ValidationError), the stack goes CREATE_FAILED then ROLLBACK_COMPLETE, and no load balancer is left behind.
Emulator: the stack reaches CREATE_COMPLETE — "expected the bad-subnet stack to roll back, but it reached CREATE_COMPLETE".
Why: the negative half of every IaC pipeline disappears: an invalid template is reported as a successful deployment, so operators never see the failure AWS would raise.
Fix: propagate resource-provider failures into stack status and run the rollback path in the CloudFormation stack handler.
Traces: elbv2.deploy-cfn-alb-nonexistent-subnet-rejected.

Cause 6 of 6: a stack UPDATE whose listener rules collide on priority is neither rejected nor rolled back (the check hangs to its deadline).
AWS: the colliding AWS::ElasticLoadBalancingV2::ListenerRule update fails with PriorityInUse, the stack ends UPDATE_ROLLBACK_COMPLETE, and the pre-update rule set survives.
Emulator: the check reaches no terminal stack status at all and times out after 60 s — no rejection, no rollback, no settled state.
Why: a client waiting on a terminal stack status waits forever; a conflicting update is silently neither applied nor refused.
Fix: priority-collision detection in the ListenerRule resource provider plus terminal UPDATE_ROLLBACK handling in the stack handler. Related open issue (CREATE path, different replay key): 20261023T025808Z-cloudformation-elbv2-a-cloudformation-stack-whose-elbv2-listener-rules-collide.md.
Traces: elbv2.deploy-elbv2-2-rule-priority-collision-update-rejected.

## Evidence
Cause 1: expected Outputs entries shaped {OutputKey, OutputValue}; actual the dict comprehension over stack["Outputs"] raises KeyError: 'OutputValue'.
Cause 2: expected DescribeTargetHealth(TargetGroupArn=<stack output>) -> 200 TargetHealthDescriptions; actual 400 TargetGroupNotFound, botocore.errorfactory.TargetGroupNotFoundException "Target group 'arn:aws:elasticloadbalancing:us-west-2:...:targetgroup/sim2real-TargetGro-.../...' not found".
Cause 3: expected DescribeServices -> one service, ListTasks -> 200; actual services=[] (IndexError: list index out of range) and ServiceNotFoundException "Service not found." on ListTasks.
Cause 4: expected DescribeTrustStores -> the created store; actual 400 TrustStoreNotFound "Trust store 'sim2real-ts-...' not found".
Cause 5: expected stack status ROLLBACK_COMPLETE; actual CREATE_COMPLETE.
Cause 6: expected a terminal UPDATE_ROLLBACK_COMPLETE (PriorityInUse on the colliding rule); actual no terminal status — "Check timed out after 60.0s".

## Steps to Reproduce
1. Cause 1: deploy any template with an Outputs section (e.g. ClusterArn/LogGroupName/TaskDefArn), then DescribeStacks and read Outputs[*].OutputValue.
2. Cause 2: deploy a template with AWS::ElasticLoadBalancingV2::TargetGroup, take the output ARN, call elbv2 DescribeTargetHealth on it.
3. Cause 3: deploy a template with AWS::ECS::Cluster + AWS::ECS::TaskDefinition + AWS::ECS::Service, then ecs DescribeServices/ListTasks for that service.
4. Cause 4: deploy a template with AWS::ElasticLoadBalancingV2::TrustStore, then elbv2 DescribeTrustStores by name.
5. Cause 5: deploy an ALB template whose Subnets reference a nonexistent subnet id and wait for a terminal stack status.
6. Cause 6: deploy two listener rules, then update the stack so two rules share one Priority and wait for a terminal stack status.
