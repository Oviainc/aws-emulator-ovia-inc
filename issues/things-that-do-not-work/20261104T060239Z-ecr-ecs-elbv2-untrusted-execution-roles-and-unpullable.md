Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.ecs-task-launch-fails-without-ecs-tasks-trust ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config ecr__ecs__elbv2.ecs-service-create-rejects-nonexistent-role-arn
Issue-Key: ecr__ecs__elbv2.ecs-task-launch-fails-without-ecs-tasks-trust|run#16|AssertionError|assert bad_task["lastStatus"] == "STOPPED", (
Issue-Key: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull|_run_and_wait#12|Timeout|time.sleep(5)
Issue-Key: ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag|_run_and_wait#11|Timeout|time.sleep(5)
Issue-Key: ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull|_run_and_wait#12|Timeout|time.sleep(5)
Issue-Key: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks|run#8|AssertionError|assert len(ips) == 1, f"expected exactly one running task ENI, got {ips!r}"
Issue-Key: ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config|_poll_until#11|Timeout|last = probe()
Issue-Key: ecr__ecs__elbv2.ecs-service-create-rejects-nonexistent-role-arn|_error#19|AssertionError|raise AssertionError(f"expected AccessDeniedException, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ecr__ecs__elbv2: untrusted execution roles and unpullable images still run, task ENIs have no private IP (no ALB targets), and RegisterTaskDefinition skips iam:PassRole (4 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Four net-new root causes, all grounded in failing behavioral checks under the suite heading `ecr__ecs__elbv2`.

Cause 1 of 4: a task whose execution role does not trust ecs-tasks.amazonaws.com reaches RUNNING.
AWS: ECS cannot assume such a role, so the task never runs: RunTask either refuses with ClientException (Smithy: ecs#ClientException is a modeled RunTask error, HTTP 400) and the message "ECS was unable to assume the role '<executionRoleArn>' that was provided for this task. Please verify that the role being passed has the proper trust relationship and permissions and that your IAM user has permissions to pass this role." (fixed text apart from <executionRoleArn>, taken from the request's task definition), or the task is created and stops with a stopped task carrying that assume-role reason. It must never report lastStatus=RUNNING.
Emulator: the task with a role trusting only ec2.amazonaws.com is accepted and DescribeTasks reports lastStatus=RUNNING (and it then never stops).
Why: a client's trust-policy misconfiguration is invisible - the estate reports a healthy running task while on AWS nothing would ever run.
Fix: RunTask validation must resolve the task definition's executionRoleArn in IAM and verify the trust relationship before creating/advancing the task (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/run_task.rs and the task state machine).
Traces: ecr__ecs__elbv2.ecs-task-launch-fails-without-ecs-tasks-trust.

Cause 2 of 4: image-pull failures never happen - a denying repository policy, a nonexistent tag and a foreign-registry image all leave the task non-terminal.
AWS: the Fargate agent authenticates to ECR with the execution role; a repository policy that denies ecr:BatchGetImage/GetDownloadUrlForLayer, an image tag that does not exist, or an image in a registry the caller cannot read makes the task stop with stopCode=TaskFailedToStart and a CannotPullContainerError stoppedReason (Smithy: ecs#Task.stopCode/stoppedReason; reason strings are recorded behavior only).
Emulator: all three checks exhaust their budget ("Check timed out after 60.0s") waiting for a terminal task: the pull is never attempted or evaluated, so a task referencing an unpullable image is indistinguishable from a healthy one.
Why: permission-boundary and image-provenance tests pass against the emulator while the corresponding AWS deployment would never start a container.
Fix: the ECS task state machine must resolve the container image against ECR (repository policy, tag existence, registry ownership) and stop the task with CannotPullContainerError (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src).
Traces: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull, ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag, ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull.

Cause 3 of 4: an awsvpc task exposes no ENI private IPv4 address, so no ALB target is ever registered.
AWS: Smithy: ecs#Task.attachments is "The Elastic Network Adapter that's associated with the task if the task uses the awsvpc network mode" and ecs#Attachment.details "[f]or elastic network interfaces ... includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address". A service created with a `loadBalancers` entry registers that privateIPv4Address in the target group with the container port, and a service created WITHOUT a loadBalancers block registers nothing.
Emulator: the service reports its running count but DescribeTasks exposes no attachment detail carrying a private IPv4 address (the check collects an empty list), and the target group never gains a target, so the positive registration fails and the negative-control check cannot be told from a service that simply never converges.
Why: an ALB in front of an ECS service is unusable - targets never appear, health checks and traffic assertions have nothing to act on.
Fix: the awsvpc task attachment synthesis in the ECS task state machine (private IPv4 from the task's subnet) and the ELBv2 target registration driven by a service's loadBalancers block (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src). The open 20261104T042502Z-autoscaling-ecs-elbv2-6-net-new-root-causes.md reports a similar symptom under another heading and does not carry these checks.
Traces: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks, ecr__ecs__elbv2.elbv2-no-target-registration-without-service-lb-config.

Cause 4 of 4: RegisterTaskDefinition does not enforce iam:PassRole on executionRoleArn.
AWS: a principal whose identity-based policy grants ECS actions but not iam:PassRole is refused when it registers a task definition carrying an executionRoleArn: AccessDeniedException (HTTP 400) with the message "User: <callerArn> is not authorized to perform: iam:PassRole on resource: <roleArn> because no identity-based policy allows the iam:PassRole action" - the fixed text is everything but <callerArn> (the calling principal) and <roleArn> (the executionRoleArn from the request). Nothing is registered, so ListTaskDefinitionFamilies(familyPrefix=<family>) stays empty.
Emulator: RegisterTaskDefinition succeeds (200) for the restricted principal and registers the family.
Why: a client relying on PassRole as the boundary that stops privilege escalation through a task role sees it silently not enforced, and the emulator registers a definition AWS would reject.
Fix: RegisterTaskDefinition's authorization path must evaluate iam:PassRole for the caller against the passed role before registering (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs).
Traces: ecr__ecs__elbv2.ecs-service-create-rejects-nonexistent-role-arn.

## Evidence
Cause 1: AssertionError: a task using an execution role that does NOT trust ecs-tasks.amazonaws.com reached lastStatus=RUNNING -- it must never reach RUNNING.
Cause 2: three checks end with "Check timed out after 60.0s" while waiting for lastStatus=STOPPED with a CannotPullContainerError stoppedReason.
Cause 3: AssertionError: expected exactly one running task ENI, got [] (no attachment detail privateIPv4Address) and no registered target in the target group; the no-loadBalancers control check times out after 60.0s.
Cause 4: AssertionError: expected AccessDeniedException, but the call SUCCEEDED and returned None - RegisterTaskDefinition by an ECS-only principal with executionRoleArn=arn:aws:iam::<account>:role/<nonexistent> returned 200 instead of AccessDeniedException "User: <userArn> is not authorized to perform: iam:PassRole on resource: <roleArn> because no identity-based policy allows the iam:PassRole action".

## Steps to Reproduce
1. Create two execution roles, one trusting ecs-tasks.amazonaws.com and one trusting ec2.amazonaws.com; register a FARGATE task definition with each; RunTask both - the ec2-trusted one is accepted and reaches RUNNING (Cause 1).
2. Push an image and (a) attach a repository policy denying ecr:BatchGetImage to the execution role, (b) reference a tag that was never pushed, (c) reference an image in another registry; RunTask each and poll DescribeTasks - no task ever stops with CannotPullContainerError (Cause 2).
3. Create a VPC/subnets/ALB/target group, create an ECS service with desiredCount=1 and a loadBalancers entry; poll DescribeTasks for the task's attachment privateIPv4Address and DescribeTargetHealth for targets - neither appears (Cause 3).
4. Create an IAM user with ECS permissions but no iam:PassRole; with its credentials call RegisterTaskDefinition with executionRoleArn naming any role - it returns 200 instead of AccessDeniedException (Cause 4).
