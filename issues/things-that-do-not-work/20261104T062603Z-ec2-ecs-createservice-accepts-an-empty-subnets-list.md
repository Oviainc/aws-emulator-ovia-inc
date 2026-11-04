Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs.ecsdeploy-awsvpc-service-with-empty-subnets-list-rejected ec2__ecs.ecsdeploy-service-security-group-from-other-vpc-rejected ec2__ecs.ecsdeploy-duplicate-active-service-name-rejected ec2__ecs.ecsdeploy-service-network-and-deployment-config-round-trips ec2__ecs.awsvpc-task-eni-matches-the-ec2-view-and-is-released-on-stop
Issue-Key: ec2__ecs.ecsdeploy-awsvpc-service-with-empty-subnets-list-rejected|_error#12|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: ec2__ecs.ecsdeploy-service-security-group-from-other-vpc-rejected|_error#13|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: ec2__ecs.ecsdeploy-duplicate-active-service-name-rejected|run#12|AssertionError|assert (code, message) == ("InvalidParameterException", "Creation of service was not idempotent."), (
Issue-Key: ec2__ecs.ecsdeploy-service-network-and-deployment-config-round-trips|run#21|AssertionError|assert cb == {"enable": True, "rollback": True, "resetOnHealthyTask": True,
Issue-Key: ec2__ecs.awsvpc-task-eni-matches-the-ec2-view-and-is-released-on-stop|_poll#5|Timeout|time.sleep(interval_s)
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# ec2__ecs: CreateService accepts an empty subnets list and a foreign-VPC security group, the duplicate-service refusal is emulator prose, circuit-breaker defaults are not materialised, and a task ENI is never visible in EC2 (5 causes)

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Cause 1 of 5: CreateService accepts awsvpcConfiguration.subnets=[].
AWS refuses with InvalidParameterException (at least one subnet is required for awsvpc). Emulator: HTTP 200, the service is created.
Fix: CreateService network-configuration validation.
Trace: ec2__ecs.ecsdeploy-awsvpc-service-with-empty-subnets-list-rejected.

Cause 2 of 5: CreateService accepts a security group that belongs to a different VPC than the subnets.
AWS refuses with InvalidParameterException. Emulator: HTTP 200.
Fix: CreateService network-configuration validation (cross-check the security group's VPC against the subnets' VPC in EC2 state).
Trace: ec2__ecs.ecsdeploy-service-security-group-from-other-vpc-rejected.

Cause 3 of 5: re-creating an ACTIVE service with different arguments is refused with the emulator's own sentence.
AWS: InvalidParameterException 'Creation of service was not idempotent.' when the same service name in the same cluster is re-created with other arguments. Emulator: InvalidParameterException 'service "<name>" already exists in cluster "<cluster>"' — emulator-authored prose (lower-case, quotes the two names).
Fix: the CreateService duplicate-name refusal message.
Trace: ec2__ecs.ecsdeploy-duplicate-active-service-name-rejected.

Cause 4 of 5: an explicitly enabled deployment circuit breaker is read back without its defaulted members.
AWS: DescribeServices returns deploymentCircuitBreaker with the full structure — enable, rollback, plus the defaulted resetOnHealthyTask and thresholdConfiguration {type, value} — beside the overridden maximumPercent/minimumHealthyPercent.
Emulator: only {'enable': True, 'rollback': True} is returned; the defaulted members are missing.
Fix: the service deploymentConfiguration defaulting/serialisation in the Create/UpdateService + DescribeServices path.
Trace: ec2__ecs.ecsdeploy-service-network-and-deployment-config-round-trips.

Cause 5 of 5: an awsvpc task's ENI is never visible in the EC2 view and never released on stop.
AWS: the task's attachment carries a networkInterfaceId that EC2 DescribeNetworkInterfaces resolves while the task runs and that disappears after it stops.
Emulator: the check polls until the 60s budget expires — the attachment never carries a networkInterfaceId (see the companion ecs attachment issue), so there is nothing to find in EC2.
Fix: the ECS awsvpc ENI allocation against EC2 state, including release on task stop.
Trace: ec2__ecs.awsvpc-task-eni-matches-the-ec2-view-and-is-released-on-stop.

## Evidence
1. `AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error`.
2. `AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code`.
3. `AssertionError: re-creating ACTIVE service 'sim2real-ecsdeploy-svc-<suffix>' with other arguments must be refused; got 'InvalidParameterException': 'service "sim2real-ecsdeploy-svc-<suffix>" already exists in cluster "sim2real-ecsdeploy-cl-<suffix>"'` (names are per-run; the expected fixed text is 'Creation of service was not idempotent.').
4. `AssertionError: an explicitly enabled circuit breaker must still materialise its reset and default threshold beside the overridden percentages; got {'enable': True, 'rollback': True}`.
5. `Error: Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateService with networkConfiguration.awsvpcConfiguration.subnets=[] -> 200.
2. CreateService with subnets from VPC A and securityGroups from VPC B -> 200.
3. CreateService name N in cluster C, then CreateService N in C again with a different taskDefinition -> wrong refusal message.
4. CreateService with deploymentCircuitBreaker {enable:true, rollback:true} and overridden percentages, then DescribeServices -> resetOnHealthyTask/thresholdConfiguration absent.
5. RunTask awsvpc, then EC2 DescribeNetworkInterfaces for the task's ENI -> never resolvable.
