Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecs-execution-role-trusts-ecs-tasks-service cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions cloudwatch__ecr__ecs.ecs-task-state-change-emits-eventbridge-event cloudwatch__ecr__ecs.ecs-task-state-change-without-matching-rule-pattern-not-delivered cloudwatch__ecr__ecs.ecs-task-stopped-reason-published-to-eventbridge cloudwatch__ecr__ecs.deploy-cfn-alarm-on-ecs-service-created-and-active cloudwatch__ecr__ecs.deploy-cfn-ecs-cluster-service-with-ecr-image-and-cloudwatch-logs cloudwatch__ecr__ecs.ecs-task-launch-fails-without-ecs-tasks-trust-policy
Issue-Key: cloudwatch__ecr__ecs.ecs-execution-role-trusts-ecs-tasks-service|_wait_task_status#24|Timeout|return last
Issue-Key: cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions|_wait_task_status#25|Timeout|return last
Issue-Key: cloudwatch__ecr__ecs.ecs-task-state-change-emits-eventbridge-event|_collect_matching#8|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Key: cloudwatch__ecr__ecs.ecs-task-state-change-without-matching-rule-pattern-not-delivered|_poll_for_event#10|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Key: cloudwatch__ecr__ecs.ecs-task-stopped-reason-published-to-eventbridge|_poll_for_event#8|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=remaining)
Issue-Key: cloudwatch__ecr__ecs.deploy-cfn-alarm-on-ecs-service-created-and-active|run#12|AssertionError|assert len(described) == 1, f"expected exactly one alarm named {alarm_name}"
Issue-Key: cloudwatch__ecr__ecs|ListTasks|ServiceNotFoundException
Issue-Key: cloudwatch__ecr__ecs.ecs-task-launch-fails-without-ecs-tasks-trust-policy|_error_code_and_message#16|AssertionError|raise AssertionError(f"expected the call to be refused; it succeeded: {result!r}")
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# cloudwatch__ecr__ecs: Fargate tasks never leave RUNNING, CFN-created alarms/services are invisible to their services, and RunTask accepts an untrusted execution role (4 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Four net-new root causes, all grounded in failing behavioral checks under the suite heading `cloudwatch__ecr__ecs`.

Cause 1 of 4: an ECS Fargate task launched by RunTask never reaches a terminal state.
AWS: Smithy: ecs#Task carries `lastStatus` ("The last known status for the task", see Task Lifecycle), plus `startedAt`, `executionStoppedAt`, `stoppedAt`, `stopCode`, `stoppedReason`, and `containers[].exitCode`. A Fargate task whose container command exits walks PROVISIONING -> PENDING -> RUNNING -> DEACTIVATING -> STOPPING -> STOPPED and DescribeTasks then reports lastStatus=STOPPED with an exitCode on each container; recorded behavior only for the timing, which the suite polls for.
Emulator: DescribeTasks keeps reporting a non-terminal task for the whole poll budget, so every check that waits for a terminal task hits the harness deadline ("Check timed out after 60.0s"). Corroborating non-timeout evidence from the sibling heading `ecr__ecs__elbv2`: in ecs-task-launch-fails-without-ecs-tasks-trust the task is reported with lastStatus=RUNNING and never advances, so tasks settle at RUNNING with no stoppedAt/stopCode/stoppedReason, no container exitCode, and no container side effects (no awslogs stream, no ECS Task State Change event).
Why: a client cannot observe the outcome of any task it launches - waiters (describe_tasks/tasks_stopped), exit-code checks, stoppedReason diagnosis and log-based assertions never complete, so every end-to-end ECS workflow hangs instead of finishing.
Fix: the ECS task state machine behind RunTask/DescribeTasks and its stored task records (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src) must drive a launched task to STOPPED and populate the stopped fields; the related open issues 20261103T150504Z-ecs-task-records-omit-platform-and-sizing-fields.md and 20261104T034544Z-cloudwatch-ecs-ssm-xray-a-launched-fargate-task.md describe neighbouring symptoms under other headings and do not carry these checks.
Traces: cloudwatch__ecr__ecs.ecs-execution-role-trusts-ecs-tasks-service, cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch, cloudwatch__ecr__ecs.ecs-task-role-assumed-for-container-permissions, cloudwatch__ecr__ecs.ecs-task-state-change-emits-eventbridge-event, cloudwatch__ecr__ecs.ecs-task-state-change-without-matching-rule-pattern-not-delivered, cloudwatch__ecr__ecs.ecs-task-stopped-reason-published-to-eventbridge.

Cause 2 of 4: a CloudFormation-created AWS::CloudWatch::Alarm is absent from CloudWatch.
AWS: a stack that reaches CREATE_COMPLETE has created every declared resource; DescribeAlarms(AlarmNames=[<the alarm declared in the template>]) then returns exactly one MetricAlarm. Recorded behavior only for the alarm's generated name.
Emulator: after the stack reports completion, DescribeAlarms returns no alarm for the declared name - the check fails with "expected exactly one alarm named <name>" (len(described) == 0).
Why: a client that provisions alarms by template sees a successful stack but has no alarm; any later PutMetricData/alarm-state assertion or dashboard read is against a resource that does not exist.
Fix: the CloudFormation resource provider for AWS::CloudWatch::Alarm must create the alarm in the cloudwatch service's state instead of only recording stack resource status (services/service-com-amazonaws-cloudformation-cloudformation/src).
Traces: cloudwatch__ecr__ecs.deploy-cfn-alarm-on-ecs-service-created-and-active.

Cause 3 of 4: a CloudFormation-created AWS::ECS::Service is unknown to ECS.
AWS: after the stack completes, ECS ListTasks(cluster=<cluster>, serviceName=<service>) succeeds for the stack's service (returning the service's task ARNs, possibly empty).
Emulator: ListTasks is refused with ServiceNotFoundException and the message "Service not found." for the service the stack just reported as created.
Why: the whole IaC path is unusable - a client deploys a service through CloudFormation and every ECS read of it (ListTasks, DescribeServices, UpdateService) fails as if it had never been created.
Fix: the CloudFormation resource provider for AWS::ECS::Service (and the cluster/task-definition resources it depends on) must register the resource in the ECS service's state (services/service-com-amazonaws-cloudformation-cloudformation/src). The open issue 20261022T234922Z-cloudwatch-ecr-ecs-a-cloudformation-stack-declaring-aws-ecr-repository-aws.md covers the ECR-repository/task-definition variant of this gap and does not carry this check.
Traces: cloudwatch__ecr__ecs.deploy-cfn-ecs-cluster-service-with-ecr-image-and-cloudwatch-logs.

Cause 4 of 4: RunTask accepts an execution role that does not trust ecs-tasks.amazonaws.com.
AWS: RunTask with a task definition whose executionRoleArn names a role whose trust policy does not list ecs-tasks.amazonaws.com is refused with ClientException (Smithy: ecs#ClientException is a modeled error of RunTask, HTTP 400) and the message "ECS was unable to assume the role '<executionRoleArn>' that was provided for this task. Please verify that the role being passed has the proper trust relationship and permissions and that your IAM user has permissions to pass this role." - the fixed text is everything but <executionRoleArn>, which is the role ARN taken from the request's task definition. No task is created.
Emulator: RunTask answers 200 and creates a task (the check's refusal probe reports "expected the call to be refused; it succeeded").
Why: a client's negative test for a mis-trusted execution role passes silently, and an estate with a broken trust relationship looks healthy instead of failing at launch.
Fix: RunTask's validation must resolve the task definition's executionRoleArn against IAM and check the role's trust policy before creating a task (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/run_task.rs).
Traces: cloudwatch__ecr__ecs.ecs-task-launch-fails-without-ecs-tasks-trust-policy.

## Evidence
Cause 1: six checks fail with "Check timed out after 60.0s" while polling DescribeTasks for lastStatus in {STOPPED} (e.g. ecs-execution-role-trusts-ecs-tasks-service waits _wait_task_status(..., {"STOPPED"}, timeout_s=240) and then asserts containers[0].exitCode == 0); the sibling check ecs-task-launch-fails-without-ecs-tasks-trust (heading ecr__ecs__elbv2) shows the task parked at lastStatus=RUNNING.
Cause 2: AssertionError: expected exactly one alarm named sim2real-xsvc-alarm-df516b0a28ec (DescribeAlarms returned none) after the stack completed.
Cause 3: botocore.errorfactory.ServiceNotFoundException: An error occurred (ServiceNotFoundException) when calling the ListTasks operation: Service not found.
Cause 4: AssertionError: expected the call to be refused; it succeeded: None - RunTask with an execution role trusting states.amazonaws.com returned 200 instead of ClientException "ECS was unable to assume the role '<arn>' ...".

## Steps to Reproduce
1. Create an ECS cluster; create an IAM role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy; RegisterTaskDefinition (FARGATE, awsvpc, cpu 256/memory 512, command ["/bin/sh","-c","echo ok && exit 0"], awslogs). RunTask and poll DescribeTasks: the task never reports lastStatus=STOPPED and no container exitCode/stoppedReason appears (Cause 1).
2. Deploy a CloudFormation template declaring AWS::CloudWatch::Alarm on an ECS service; after the stack completes call DescribeAlarms with the declared alarm name - nothing is returned (Cause 2).
3. Deploy a CloudFormation template declaring AWS::ECS::Cluster + AWS::ECS::TaskDefinition + AWS::ECS::Service; after completion call ecs ListTasks(cluster, serviceName) - ServiceNotFoundException "Service not found." (Cause 3).
4. Create an IAM role whose trust policy names states.amazonaws.com, register a FARGATE task definition with it as executionRoleArn, then RunTask - the call returns 200 with a task instead of ClientException (Cause 4).
