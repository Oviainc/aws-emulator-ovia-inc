Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.run-task-client-token-conflict-rejected ecs.run-task-client-token-idempotency ecs.run-task-client-token-idempotency-concurrent ecs.run-task-client-token-idempotent-concurrent ecs.capprov-base-and-weight-place-containers-that-really-run ecs.capacity-provider-strategy-fallback-ordering ecs.capacity-provider-strategy-fargate-spot-task-placement ecs.run-task-propagate-tags-from-task-definition ecs.task-protection-get-reflects-update ecs.deploy-cfn-service-desired-count-materializes-tasks ecs.deploy-cfn-taskdefinition-revision-used-by-service-tasks ecs.deploy-ecs-service-cfn-reaches-steady-state ecs.deploy-ecs-service-cfn-update-triggers-new-deployment ecs.deploy-ecs-cluster-service-cfn-reaches-steady-state ecs.capacity-provider-create-update-delete-lifecycle ecs.capacity-provider-lifecycle-active-to-inactive ecs.capacity-provider-update-changes-managed-scaling ecs.update-capacity-provider-after-delete-fails ecs.delete-cluster-with-active-capacity-providers-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.run-task-client-token-conflict-rejected|run#16|AssertionError|assert replay["tasks"][0]["taskArn"] == task_arn, (
Issue-Key: ecs.run-task-client-token-idempotency|run#25|AssertionError|assert second["tasks"][0]["taskArn"] == task_arn, (
Issue-Key: ecs.run-task-client-token-idempotency-concurrent|run#16|AssertionError|assert len(all_task_arns) == 1, (
Issue-Key: ecs.run-task-client-token-idempotent-concurrent|run#11|AssertionError|assert len(unique_arns) < n_racers, (
Issue-Key: ecs.capprov-base-and-weight-place-containers-that-really-run|run#13|AssertionError|assert placed == Counter({"FARGATE": 2, "FARGATE_SPOT": 1}), (
Issue-Key: ecs.capacity-provider-strategy-fallback-ordering|run#13|AssertionError|assert providers == ["FARGATE", "FARGATE_SPOT"], (
Issue-Key: ecs.capacity-provider-strategy-fargate-spot-task-placement|run#12|AssertionError|assert task.get("launchType") == "FARGATE", (
Issue-Key: ecs.run-task-propagate-tags-from-task-definition|run#19|AssertionError|assert got.get("aws:ecs:clusterName") == cluster, (
Issue-Key: ecs.task-protection-get-reflects-update|_poll_until#18|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last}")
Issue-Key: ecs|ListTasks|ServiceNotFoundException
Issue-Key: ecs.deploy-ecs-service-cfn-reaches-steady-state|_describe#11|IndexError|return ecs.describe_services(cluster=cluster_arn, services=[service_arn])["services"][0]
Issue-Key: ecs.deploy-ecs-service-cfn-update-triggers-new-deployment|_describe#12|IndexError|return ecs.describe_services(cluster=cluster_arn, services=[service_arn])["services"][0]
Issue-Key: ecs.deploy-ecs-cluster-service-cfn-reaches-steady-state|run#12|Timeout|time.sleep(5.0)
Issue-Key: ecs.delete-cluster-with-active-capacity-providers-rejected|_find_ami#5|AssertionError|raise AssertionError("no Amazon Linux 2 AMI available to back a capacity-provider Auto Scaling group")
Issue-Related: 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ECS: RunTask clientToken is ignored, capacity-provider base/weight is not honoured, ECS managed tags are not added, GetTaskProtection does not reflect an update, CFN-created services are invisible, and the ECS-optimized AMI cannot be resolved (6 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 6: RunTask's clientToken is never honoured.
AWS: Smithy com.amazonaws.ecs#RunTaskRequest.clientToken carries the @idempotencyToken trait — "An identifier that you provide to ensure the idempotency of the request". Re-running RunTask with the same clientToken and identical parameters returns the SAME task; with the same token and different parameters AWS refuses the request as non-idempotent.
Emulator: every call mints a new task ARN. Sequential reuse returns a second task, and five/six concurrent racers sharing one token produce five/six distinct task ARNs.
Why: a retrying client (SDK retry, step-function retry, CI re-run) silently double-launches work.
Fix: RunTask handler/state — a per-(account,region,cluster,clientToken) dedup record.
Traces: ecs.run-task-client-token-conflict-rejected, ecs.run-task-client-token-idempotency, ecs.run-task-client-token-idempotency-concurrent, ecs.run-task-client-token-idempotent-concurrent.

Cause 2 of 6: capacityProviderStrategy base/weight is ignored and the FARGATE_SPOT task reports no launchType.
AWS: within a strategy, `base` places that many tasks on the provider first and `weight` distributes the remainder proportionally; a task placed through the FARGATE_SPOT provider reports launchType=FARGATE (the underlying platform) and capacityProviderName=FARGATE_SPOT.
Emulator: all tasks land on one provider regardless of base/weight (3 tasks reported as {'FARGATE_SPOT': 3} where base=1 on FARGATE_SPOT plus all weight on FARGATE must split 1/2; and a fallback case produced ['FARGATE','FARGATE']), and the spot-placed task carries launchType=None.
Fix: the capacity-provider placement logic in the RunTask/service scheduler, and the launchType projection on the task.
Traces: ecs.capprov-base-and-weight-place-containers-that-really-run, ecs.capacity-provider-strategy-fallback-ordering, ecs.capacity-provider-strategy-fargate-spot-task-placement.

Cause 3 of 6: enableECSManagedTags does not add the managed tags.
AWS: with enableECSManagedTags=true the task carries the managed tag aws:ecs:clusterName whose value is the cluster name (alongside any propagated tags).
Emulator: the task's tags contain only the user/propagated tags; no aws:ecs:* tag is added.
Fix: the tag-materialisation step of the RunTask/CreateService handler.
Trace: ecs.run-task-propagate-tags-from-task-definition.

Cause 4 of 6: GetTaskProtection does not reflect a preceding UpdateTaskProtection.
AWS: after UpdateTaskProtection(protectionEnabled=true[, expiresInMinutes]) GetTaskProtection returns that task with protectionEnabled=true and its expiration.
Emulator: the read never shows the updated protection within the poll window.
Fix: the task-protection state write/read path.
Trace: ecs.task-protection-get-reflects-update.

Cause 5 of 6: a CloudFormation-created AWS::ECS::Service/TaskDefinition is invisible to ECS.
AWS: once the stack reports the resource created, ECS reads (ListTasks/DescribeServices/DescribeTaskDefinition) resolve it.
Emulator: ListTasks for the stack's service answers ServiceNotFoundException 'Service not found.', DescribeTaskDefinition answers ClientException 'Unable to describe task definition.', and the stack's resource/output lookups come back empty (IndexError in the check as it indexes an empty list).
Fix: the CloudFormation ECS resource providers must create the resource in the ECS service's own state (not only in stack bookkeeping).
Traces: ecs.deploy-cfn-service-desired-count-materializes-tasks, ecs.deploy-cfn-taskdefinition-revision-used-by-service-tasks, ecs.deploy-ecs-service-cfn-reaches-steady-state, ecs.deploy-ecs-service-cfn-update-triggers-new-deployment, ecs.deploy-ecs-cluster-service-cfn-reaches-steady-state.

Cause 6 of 6: the ECS-optimized AMI cannot be resolved, so no EC2-backed capacity provider can be built.
AWS: SSM serves the public parameter path /aws/service/ecs/optimized-ami/... and EC2 DescribeImages returns Amazon-published Amazon Linux 2 AMIs, which is how an ECS capacity-provider Auto Scaling group is normally created.
Emulator: GetParameter answers ParameterNotFound (with an empty message) and DescribeImages reports no Amazon Linux 2 AMI, so five capacity-provider lifecycle checks cannot even provision their subject.
Fix: the SSM public-parameter seed and the EC2 DescribeImages image catalogue.
Traces: ecs.capacity-provider-create-update-delete-lifecycle, ecs.capacity-provider-lifecycle-active-to-inactive, ecs.capacity-provider-update-changes-managed-scaling, ecs.update-capacity-provider-after-delete-fails, ecs.delete-cluster-with-active-capacity-providers-rejected.

## Evidence
1. `AssertionError: reusing the same clientToken with identical params must return the SAME task, got arn:aws:ecs:...:task/<cluster>/...0002 vs .../...0001`; `AssertionError: expected all 5 concurrent calls with the same clientToken to dedupe to a single task, but observed 5 distinct task ARNs: {...}`; `AssertionError: expected clientToken idempotency to dedupe at least one of 6 concurrent racers ... but every racer launched a distinct task`.
2. `AssertionError: base=1 on FARGATE_SPOT plus all weight on FARGATE must split 3 tasks 1/2; RunTask reported {'FARGATE_SPOT': 3}`; `AssertionError: expected exactly one task on FARGATE (the base allocation) and one falling back to FARGATE_SPOT ..., got ['FARGATE', 'FARGATE']`; `AssertionError: expected launchType=FARGATE (the underlying platform for the FARGATE_SPOT capacity provider), got None`.
3. `AssertionError: enableECSManagedTags=True must add the managed aws:ecs:clusterName tag with the cluster name as its value, got {'sim2real-propagated': 'yes', 'sim2real-owner': 'simparity'}`.
4. `AssertionError: condition not met within 25.0s; last=False`.
5. `botocore.errorfactory.ServiceNotFoundException: An error occurred (ServiceNotFoundException) when calling the ListTasks operation: Service not found.`; `botocore.errorfactory.ClientException: An error occurred (ClientException) when calling the DescribeTaskDefinition operation: Unable to describe task definition.`; `IndexError: list index out of range`.
6. `AssertionError: could not resolve a default AMI via SSM: An error occurred (ParameterNotFound) when calling the GetParameter operation:`; `AssertionError: no Amazon Linux 2 AMI available to back a capacity-provider Auto Scaling group`.

## Steps to Reproduce
1. RunTask twice with the same clientToken and identical parameters -> two task ARNs; then 5 concurrent calls with one token -> 5 ARNs.
2. Associate FARGATE and FARGATE_SPOT, RunTask count=3 with base=1 on FARGATE_SPOT and weight only on FARGATE -> all three land on FARGATE_SPOT; the spot task reports launchType=None.
3. RunTask with enableECSManagedTags=true and propagateTags=TASK_DEFINITION, then read the task's tags -> no aws:ecs:clusterName.
4. UpdateTaskProtection(protectionEnabled=true) then GetTaskProtection -> not reflected.
5. CreateStack with AWS::ECS::Cluster/TaskDefinition/Service, wait for the stack, then ListTasks(service=<stack service>) -> ServiceNotFoundException.
6. SSM GetParameter('/aws/service/ecs/optimized-ami/amazon-linux-2/recommended/image_id') -> ParameterNotFound; EC2 DescribeImages filtered to amazon-owned AL2 -> empty.
